use alloy_primitives::{keccak256, B256};
use itertools::Itertools;
use reth_config::config::{EtlConfig, HashingConfig};
use reth_db::{tables, RawKey, RawTable, RawValue};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    transaction::{DbTx, DbTxMut},
};
use reth_etl::Collector;
use reth_primitives::Account;
use reth_provider::{AccountExtReader, DBProvider, HashingWriter, StatsReader};
use reth_stages_api::{
    AccountHashingCheckpoint, EntitiesCheckpoint, ExecInput, ExecOutput, Stage, StageCheckpoint,
    StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_storage_errors::provider::ProviderResult;
use std::{
    fmt::Debug,
    ops::{Range, RangeInclusive},
    sync::mpsc::{self, Receiver},
};
use tracing::*;

/// Maximum number of channels that can exist in memory.
const MAXIMUM_CHANNELS: usize = 10_000;

/// Maximum number of accounts to hash per rayon worker job.
const WORKER_CHUNK_SIZE: usize = 100;

/// Account hashing stage hashes plain account.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Clone, Debug)]
pub struct AccountHashingStage {
    /// The threshold (in number of blocks) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of accounts to process before committing during unwind.
    pub commit_threshold: u64,
    /// ETL configuration
    pub etl_config: EtlConfig,
}

impl AccountHashingStage {
    /// Create new instance of [`AccountHashingStage`].
    pub const fn new(config: HashingConfig, etl_config: EtlConfig) -> Self {
        Self {
            clean_threshold: config.clean_threshold,
            commit_threshold: config.commit_threshold,
            etl_config,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl AccountHashingStage {
    /// Initializes the `PlainAccountState` table with `num_accounts` having some random state
    /// at the target block, with `txs_range` transactions in each block.
    ///
    /// Proceeds to go to the `BlockTransitionIndex` end, go back `transitions` and change the
    /// account state in the `AccountChangeSets` table.
    pub fn seed<
        Tx: DbTx + DbTxMut + 'static,
        Spec: Send + Sync + 'static + reth_chainspec::EthereumHardforks,
    >(
        provider: &reth_provider::DatabaseProvider<Tx, Spec>,
        opts: SeedOpts,
    ) -> Result<Vec<(alloy_primitives::Address, reth_primitives::Account)>, StageError> {
        use alloy_primitives::U256;
        use reth_db_api::models::AccountBeforeTx;
        use reth_provider::{StaticFileProviderFactory, StaticFileWriter};
        use reth_testing_utils::{
            generators,
            generators::{random_block_range, random_eoa_accounts, BlockRangeParams},
        };

        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            opts.blocks.clone(),
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: opts.txs, ..Default::default() },
        );

        for block in blocks {
            provider.insert_historical_block(block.try_seal_with_senders().unwrap()).unwrap();
        }
        provider
            .static_file_provider()
            .latest_writer(reth_primitives::StaticFileSegment::Headers)
            .unwrap()
            .commit()
            .unwrap();
        let mut accounts = random_eoa_accounts(&mut rng, opts.accounts);
        {
            // Account State generator
            let mut account_cursor =
                provider.tx_ref().cursor_write::<tables::PlainAccountState>()?;
            accounts.sort_by(|a, b| a.0.cmp(&b.0));
            for (addr, acc) in &accounts {
                account_cursor.append(*addr, *acc)?;
            }

            let mut acc_changeset_cursor =
                provider.tx_ref().cursor_write::<tables::AccountChangeSets>()?;
            for (t, (addr, acc)) in opts.blocks.zip(&accounts) {
                let Account { nonce, balance, .. } = acc;
                let prev_acc = Account {
                    nonce: nonce - 1,
                    balance: balance - U256::from(1),
                    bytecode_hash: None,
                };
                let acc_before_tx = AccountBeforeTx { address: *addr, info: Some(prev_acc) };
                acc_changeset_cursor.append(t, acc_before_tx)?;
            }
        }

        Ok(accounts)
    }
}

impl Default for AccountHashingStage {
    fn default() -> Self {
        Self {
            clean_threshold: 500_000,
            commit_threshold: 100_000,
            etl_config: EtlConfig::default(),
        }
    }
}

impl<Provider> Stage<Provider> for AccountHashingStage
where
    Provider: DBProvider<Tx: DbTxMut> + HashingWriter + AccountExtReader + StatsReader,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::AccountHashing
    }

    /// Execute the stage.
    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let (from_block, to_block) = input.next_block_range().into_inner();

        // if there are more blocks then threshold it is faster to go over Plain state and hash all
        // account otherwise take changesets aggregate the sets and apply hashing to
        // AccountHashing table. Also, if we start from genesis, we need to hash from scratch, as
        // genesis accounts are not in changeset.
        if to_block - from_block > self.clean_threshold || from_block == 1 {
            let tx = provider.tx_ref();

            // clear table, load all accounts and hash it
            tx.clear::<tables::HashedAccounts>()?;

            let mut accounts_cursor = tx.cursor_read::<RawTable<tables::PlainAccountState>>()?;
            let mut collector =
                Collector::new(self.etl_config.file_size, self.etl_config.dir.clone());
            let mut channels = Vec::with_capacity(MAXIMUM_CHANNELS);

            // channels used to return result of account hashing
            for chunk in &accounts_cursor.walk(None)?.chunks(WORKER_CHUNK_SIZE) {
                // An _unordered_ channel to receive results from a rayon job
                let (tx, rx) = mpsc::channel();
                channels.push(rx);

                let chunk = chunk.collect::<Result<Vec<_>, _>>()?;
                // Spawn the hashing task onto the global rayon pool
                rayon::spawn(move || {
                    for (address, account) in chunk {
                        let address = address.key().unwrap();
                        let _ = tx.send((RawKey::new(keccak256(address)), account));
                    }
                });

                // Flush to ETL when channels length reaches MAXIMUM_CHANNELS
                if !channels.is_empty() && channels.len() % MAXIMUM_CHANNELS == 0 {
                    collect(&mut channels, &mut collector)?;
                }
            }

            collect(&mut channels, &mut collector)?;

            let mut hashed_account_cursor =
                tx.cursor_write::<RawTable<tables::HashedAccounts>>()?;

            let total_hashes = collector.len();
            let interval = (total_hashes / 10).max(1);
            for (index, item) in collector.iter()?.enumerate() {
                if index > 0 && index % interval == 0 {
                    info!(
                        target: "sync::stages::hashing_account",
                        progress = %format!("{:.2}%", (index as f64 / total_hashes as f64) * 100.0),
                        "Inserting hashes"
                    );
                }

                let (key, value) = item?;
                hashed_account_cursor
                    .append(RawKey::<B256>::from_vec(key), RawValue::<Account>::from_vec(value))?;
            }
        } else {
            // Aggregate all transition changesets and make a list of accounts that have been
            // changed.
            let lists = provider.changed_accounts_with_range(from_block..=to_block)?;
            // Iterate over plain state and get newest value.
            // Assumption we are okay to make is that plainstate represent
            // `previous_stage_progress` state.
            let accounts = provider.basic_accounts(lists)?;
            // Insert and hash accounts to hashing table
            provider.insert_account_for_hashing(accounts)?;
        }

        // We finished the hashing stage, no future iterations is expected for the same block range,
        // so no checkpoint is needed.
        let checkpoint = StageCheckpoint::new(input.target())
            .with_account_hashing_stage_checkpoint(AccountHashingCheckpoint {
                progress: stage_checkpoint_progress(provider)?,
                ..Default::default()
            });

        Ok(ExecOutput { checkpoint, done: true })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        // Aggregate all transition changesets and make a list of accounts that have been changed.
        provider.unwind_account_hashing(range)?;

        let mut stage_checkpoint =
            input.checkpoint.account_hashing_stage_checkpoint().unwrap_or_default();

        stage_checkpoint.progress = stage_checkpoint_progress(provider)?;

        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(unwind_progress)
                .with_account_hashing_stage_checkpoint(stage_checkpoint),
        })
    }
}

/// Flushes channels hashes to ETL collector.
fn collect(
    channels: &mut Vec<Receiver<(RawKey<B256>, RawValue<Account>)>>,
    collector: &mut Collector<RawKey<B256>, RawValue<Account>>,
) -> Result<(), StageError> {
    for channel in channels.iter_mut() {
        while let Ok((key, v)) = channel.recv() {
            collector.insert(key, v)?;
        }
    }
    info!(target: "sync::stages::hashing_account", "Hashed {} entries", collector.len());
    channels.clear();
    Ok(())
}

// TODO: Rewrite this
/// `SeedOpts` provides configuration parameters for calling `AccountHashingStage::seed`
/// in unit tests or benchmarks to generate an initial database state for running the
/// stage.
///
/// In order to check the "full hashing" mode of the stage you want to generate more
/// transitions than `AccountHashingStage.clean_threshold`. This requires:
/// 1. Creating enough blocks so there's enough transactions to generate the required transition
///    keys in the `BlockTransitionIndex` (which depends on the `TxTransitionIndex` internally)
/// 2. Setting `blocks.len() > clean_threshold` so that there's enough diffs to actually take the
///    2nd codepath
#[derive(Clone, Debug)]
pub struct SeedOpts {
    /// The range of blocks to be generated
    pub blocks: RangeInclusive<u64>,
    /// The number of accounts to be generated
    pub accounts: usize,
    /// The range of transactions to be generated per block.
    pub txs: Range<u8>,
}

fn stage_checkpoint_progress(provider: &impl StatsReader) -> ProviderResult<EntitiesCheckpoint> {
    Ok(EntitiesCheckpoint {
        processed: provider.count_entries::<tables::HashedAccounts>()? as u64,
        total: provider.count_entries::<tables::PlainAccountState>()? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        UnwindStageTestRunner,
    };
    use alloy_primitives::U256;
    use assert_matches::assert_matches;
    use reth_primitives::Account;
    use reth_provider::providers::StaticFileWriter;
    use reth_stages_api::StageUnitCheckpoint;
    use test_utils::*;

    stage_test_suite_ext!(AccountHashingTestRunner, account_hashing);

    #[tokio::test]
    async fn execute_clean_account_hashing() {
        let (previous_stage, stage_progress) = (20, 10);
        // Set up the runner
        let mut runner = AccountHashingTestRunner::default();
        runner.set_clean_threshold(1);

        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Account(AccountHashingCheckpoint {
                        progress: EntitiesCheckpoint {
                            processed,
                            total,
                        },
                        ..
                    })),
                },
                done: true,
            }) if block_number == previous_stage &&
                processed == total &&
                total == runner.db.table::<tables::PlainAccountState>().unwrap().len() as u64
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    mod test_utils {
        use super::*;
        use crate::test_utils::TestStageDB;
        use alloy_primitives::Address;
        use reth_provider::DatabaseProviderFactory;

        pub(crate) struct AccountHashingTestRunner {
            pub(crate) db: TestStageDB,
            commit_threshold: u64,
            clean_threshold: u64,
            etl_config: EtlConfig,
        }

        impl AccountHashingTestRunner {
            pub(crate) fn set_clean_threshold(&mut self, threshold: u64) {
                self.clean_threshold = threshold;
            }

            #[allow(dead_code)]
            pub(crate) fn set_commit_threshold(&mut self, threshold: u64) {
                self.commit_threshold = threshold;
            }

            /// Iterates over `PlainAccount` table and checks that the accounts match the ones
            /// in the `HashedAccounts` table
            pub(crate) fn check_hashed_accounts(&self) -> Result<(), TestRunnerError> {
                self.db.query(|tx| {
                    let mut acc_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut hashed_acc_cursor = tx.cursor_read::<tables::HashedAccounts>()?;

                    while let Some((address, account)) = acc_cursor.next()? {
                        let hashed_addr = keccak256(address);
                        if let Some((_, acc)) = hashed_acc_cursor.seek_exact(hashed_addr)? {
                            assert_eq!(acc, account)
                        }
                    }
                    Ok(())
                })?;

                Ok(())
            }

            /// Same as `check_hashed_accounts`, only that checks with the old account state,
            /// namely, the same account with nonce - 1 and balance - 1.
            pub(crate) fn check_old_hashed_accounts(&self) -> Result<(), TestRunnerError> {
                self.db.query(|tx| {
                    let mut acc_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut hashed_acc_cursor = tx.cursor_read::<tables::HashedAccounts>()?;

                    while let Some((address, account)) = acc_cursor.next()? {
                        let Account { nonce, balance, .. } = account;
                        let old_acc = Account {
                            nonce: nonce - 1,
                            balance: balance - U256::from(1),
                            bytecode_hash: None,
                        };
                        let hashed_addr = keccak256(address);
                        if let Some((_, acc)) = hashed_acc_cursor.seek_exact(hashed_addr)? {
                            assert_eq!(acc, old_acc)
                        }
                    }
                    Ok(())
                })?;

                Ok(())
            }
        }

        impl Default for AccountHashingTestRunner {
            fn default() -> Self {
                Self {
                    db: TestStageDB::default(),
                    commit_threshold: 1000,
                    clean_threshold: 1000,
                    etl_config: EtlConfig::default(),
                }
            }
        }

        impl StageTestRunner for AccountHashingTestRunner {
            type S = AccountHashingStage;

            fn db(&self) -> &TestStageDB {
                &self.db
            }

            fn stage(&self) -> Self::S {
                Self::S {
                    commit_threshold: self.commit_threshold,
                    clean_threshold: self.clean_threshold,
                    etl_config: self.etl_config.clone(),
                }
            }
        }

        impl ExecuteStageTestRunner for AccountHashingTestRunner {
            type Seed = Vec<(Address, Account)>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let provider = self.db.factory.database_provider_rw()?;
                let res = Ok(AccountHashingStage::seed(
                    &provider,
                    SeedOpts { blocks: 1..=input.target(), accounts: 10, txs: 0..3 },
                )
                .unwrap());
                provider.commit().expect("failed to commit");
                res
            }

            fn validate_execution(
                &self,
                input: ExecInput,
                output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                if let Some(output) = output {
                    let start_block = input.next_block();
                    let end_block = output.checkpoint.block_number;
                    if start_block > end_block {
                        return Ok(())
                    }
                }
                self.check_hashed_accounts()
            }
        }

        impl UnwindStageTestRunner for AccountHashingTestRunner {
            fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
                self.check_old_hashed_accounts()
            }
        }
    }
}
