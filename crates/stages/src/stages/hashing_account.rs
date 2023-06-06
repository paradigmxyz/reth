use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use itertools::Itertools;
use rayon::slice::ParallelSliceMut;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable,
};
use reth_interfaces::db::DatabaseError;
use reth_primitives::{
    keccak256,
    stage::{
        AccountHashingCheckpoint, CheckpointBlockRange, EntitiesCheckpoint, StageCheckpoint,
        StageId,
    },
};
use reth_provider::Transaction;
use std::{
    cmp::max,
    fmt::Debug,
    ops::{Deref, Range, RangeInclusive},
};
use tokio::sync::mpsc;
use tracing::*;

/// Account hashing stage hashes plain account.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Clone, Debug)]
pub struct AccountHashingStage {
    /// The threshold (in number of state transitions) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of accounts to process before committing.
    pub commit_threshold: u64,
}

impl AccountHashingStage {
    /// Create new instance of [AccountHashingStage].
    pub fn new(clean_threshold: u64, commit_threshold: u64) -> Self {
        Self { clean_threshold, commit_threshold }
    }
}

impl Default for AccountHashingStage {
    fn default() -> Self {
        Self { clean_threshold: 500_000, commit_threshold: 100_000 }
    }
}

// TODO: Rewrite this
/// `SeedOpts` provides configuration parameters for calling `AccountHashingStage::seed`
/// in unit tests or benchmarks to generate an initial database state for running the
/// stage.
///
/// In order to check the "full hashing" mode of the stage you want to generate more
/// transitions than `AccountHashingStage.clean_threshold`. This requires:
/// 1. Creating enough blocks so there's enough transactions to generate
/// the required transition keys in the `BlockTransitionIndex` (which depends on the
/// `TxTransitionIndex` internally)
/// 2. Setting `blocks.len() > clean_threshold` so that there's enough diffs to actually
/// take the 2nd codepath
#[derive(Clone, Debug)]
pub struct SeedOpts {
    /// The range of blocks to be generated
    pub blocks: RangeInclusive<u64>,
    /// The range of accounts to be generated
    pub accounts: Range<u64>,
    /// The range of transactions to be generated per block.
    pub txs: Range<u8>,
}

#[cfg(any(test, feature = "test-utils"))]
impl AccountHashingStage {
    /// Initializes the `PlainAccountState` table with `num_accounts` having some random state
    /// at the target block, with `txs_range` transactions in each block.
    ///
    /// Proceeds to go to the `BlockTransitionIndex` end, go back `transitions` and change the
    /// account state in the `AccountChangeSet` table.
    pub fn seed<DB: Database>(
        tx: &mut Transaction<'_, DB>,
        opts: SeedOpts,
    ) -> Result<Vec<(reth_primitives::Address, reth_primitives::Account)>, StageError> {
        use reth_db::models::AccountBeforeTx;
        use reth_interfaces::test_utils::generators::{
            random_block_range, random_eoa_account_range,
        };
        use reth_primitives::{Account, H256, U256};
        use reth_provider::insert_canonical_block;

        let blocks = random_block_range(opts.blocks.clone(), H256::zero(), opts.txs);

        for block in blocks {
            insert_canonical_block(&**tx, block, None).unwrap();
        }
        let mut accounts = random_eoa_account_range(opts.accounts);
        {
            // Account State generator
            let mut account_cursor = tx.cursor_write::<tables::PlainAccountState>()?;
            accounts.sort_by(|a, b| a.0.cmp(&b.0));
            for (addr, acc) in accounts.iter() {
                account_cursor.append(*addr, *acc)?;
            }

            let mut acc_changeset_cursor = tx.cursor_write::<tables::AccountChangeSet>()?;
            for (t, (addr, acc)) in (opts.blocks).zip(&accounts) {
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

        tx.commit()?;

        Ok(accounts)
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for AccountHashingStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::AccountHashing
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let range = input.next_block_range();
        if range.is_empty() {
            return Ok(ExecOutput::done(*range.end()))
        }
        let (from_block, to_block) = range.into_inner();

        // if there are more blocks then threshold it is faster to go over Plain state and hash all
        // account otherwise take changesets aggregate the sets and apply hashing to
        // AccountHashing table. Also, if we start from genesis, we need to hash from scratch, as
        // genesis accounts are not in changeset.
        if to_block - from_block > self.clean_threshold || from_block == 1 {
            let stage_checkpoint = input
                .checkpoint
                .and_then(|checkpoint| checkpoint.account_hashing_stage_checkpoint());

            let start_address = match stage_checkpoint {
                Some(AccountHashingCheckpoint { address: address @ Some(_), block_range: CheckpointBlockRange { from, to }, .. })
                    // Checkpoint is only valid if the range of transitions didn't change.
                    // An already hashed account may have been changed with the new range,
                    // and therefore should be hashed again.
                    if from == from_block && to == to_block =>
                {
                    debug!(target: "sync::stages::account_hashing::exec", checkpoint = ?stage_checkpoint, "Continuing inner account hashing checkpoint");

                    address
                }
                _ => {
                    // clear table, load all accounts and hash it
                    tx.clear::<tables::HashedAccount>()?;

                    None
                }
            }
            .take()
            .map(RawKey::new);

            let next_address = {
                let mut accounts_cursor =
                    tx.cursor_read::<RawTable<tables::PlainAccountState>>()?;

                // channels used to return result of account hashing
                let mut channels = Vec::new();
                for chunk in &accounts_cursor
                    .walk(start_address.clone())?
                    .take(self.commit_threshold as usize)
                    .chunks(
                        max(self.commit_threshold as usize, rayon::current_num_threads()) /
                            rayon::current_num_threads(),
                    )
                {
                    // An _unordered_ channel to receive results from a rayon job
                    let (tx, rx) = mpsc::unbounded_channel();
                    channels.push(rx);

                    let chunk = chunk.collect::<Result<Vec<_>, _>>()?;
                    // Spawn the hashing task onto the global rayon pool
                    rayon::spawn(move || {
                        for (address, account) in chunk.into_iter() {
                            let address = address.key().unwrap();
                            let _ = tx.send((RawKey::new(keccak256(address)), account));
                        }
                    });
                }
                let mut hashed_batch = Vec::with_capacity(self.commit_threshold as usize);

                // Iterate over channels and append the hashed accounts.
                for mut channel in channels {
                    while let Some(hashed) = channel.recv().await {
                        hashed_batch.push(hashed);
                    }
                }
                // sort it all in parallel
                hashed_batch.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));

                let mut hashed_account_cursor =
                    tx.cursor_write::<RawTable<tables::HashedAccount>>()?;

                // iterate and put presorted hashed accounts
                if start_address.is_none() {
                    hashed_batch
                        .into_iter()
                        .try_for_each(|(k, v)| hashed_account_cursor.append(k, v))?;
                } else {
                    hashed_batch
                        .into_iter()
                        .try_for_each(|(k, v)| hashed_account_cursor.insert(k, v))?;
                }
                // next key of iterator
                accounts_cursor.next()?
            };

            if let Some((next_address, _)) = &next_address {
                let checkpoint = input.checkpoint().with_account_hashing_stage_checkpoint(
                    AccountHashingCheckpoint {
                        address: Some(next_address.key().unwrap()),
                        block_range: CheckpointBlockRange { from: from_block, to: to_block },
                        progress: stage_checkpoint_progress(tx)?,
                    },
                );

                info!(target: "sync::stages::hashing_account", checkpoint = %checkpoint, is_final_range = false, "Stage iteration finished");
                return Ok(ExecOutput { checkpoint, done: false })
            }
        } else {
            // Aggregate all transition changesets and make a list of accounts that have been
            // changed.
            let lists = tx.get_addresses_of_changed_accounts(from_block..=to_block)?;
            // Iterate over plain state and get newest value.
            // Assumption we are okay to make is that plainstate represent
            // `previous_stage_progress` state.
            let accounts = tx.get_plainstate_accounts(lists)?;
            // Insert and hash accounts to hashing table
            tx.insert_account_for_hashing(accounts.into_iter())?;
        }

        // We finished the hashing stage, no future iterations is expected for the same block range,
        // so no checkpoint is needed.
        let checkpoint = StageCheckpoint::new(input.previous_stage_checkpoint_block_number())
            .with_account_hashing_stage_checkpoint(AccountHashingCheckpoint {
                progress: stage_checkpoint_progress(tx)?,
                ..Default::default()
            });

        info!(target: "sync::stages::hashing_account", checkpoint = %checkpoint, is_final_range = true, "Stage iteration finished");
        Ok(ExecOutput { checkpoint, done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, is_final_range) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        // Aggregate all transition changesets and make a list of accounts that have been changed.
        tx.unwind_account_hashing(range)?;

        let mut stage_checkpoint =
            input.checkpoint.account_hashing_stage_checkpoint().unwrap_or_default();

        stage_checkpoint.progress = stage_checkpoint_progress(tx)?;

        info!(target: "sync::stages::hashing_account", to_block = input.unwind_to, %unwind_progress, is_final_range, "Unwind iteration finished");
        Ok(UnwindOutput {
            checkpoint: StageCheckpoint::new(unwind_progress)
                .with_account_hashing_stage_checkpoint(stage_checkpoint),
        })
    }
}

fn stage_checkpoint_progress<DB: Database>(
    tx: &Transaction<'_, DB>,
) -> Result<EntitiesCheckpoint, DatabaseError> {
    Ok(EntitiesCheckpoint {
        processed: tx.deref().entries::<tables::HashedAccount>()? as u64,
        total: tx.deref().entries::<tables::PlainAccountState>()? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, TestRunnerError, UnwindStageTestRunner,
        PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_primitives::{stage::StageUnitCheckpoint, Account, U256};
    use test_utils::*;

    stage_test_suite_ext!(AccountHashingTestRunner, account_hashing);

    #[tokio::test]
    async fn execute_clean_account_hashing() {
        let (previous_stage, stage_progress) = (20, 10);
        // Set up the runner
        let mut runner = AccountHashingTestRunner::default();
        runner.set_clean_threshold(1);

        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
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
                total == runner.tx.table::<tables::PlainAccountState>().unwrap().len() as u64
        );

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    #[tokio::test]
    async fn execute_clean_account_hashing_with_commit_threshold() {
        let (previous_stage, stage_progress) = (20, 10);
        // Set up the runner
        let mut runner = AccountHashingTestRunner::default();
        runner.set_clean_threshold(1);
        runner.set_commit_threshold(5);

        let mut input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, previous_stage)),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        // first run, hash first five accounts.
        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        let fifth_address = runner
            .tx
            .query(|tx| {
                let (address, _) = tx
                    .cursor_read::<tables::PlainAccountState>()?
                    .walk(None)?
                    .nth(5)
                    .unwrap()
                    .unwrap();
                Ok(address)
            })
            .unwrap();

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number: 10,
                    stage_checkpoint: Some(StageUnitCheckpoint::Account(
                        AccountHashingCheckpoint {
                            address: Some(address),
                            block_range: CheckpointBlockRange {
                                from: 11,
                                to: 20,
                            },
                            progress: EntitiesCheckpoint { processed: 5, total }
                        }
                    ))
                },
                done: false
            }) if address == fifth_address &&
                total == runner.tx.table::<tables::PlainAccountState>().unwrap().len() as u64
        );
        assert_eq!(runner.tx.table::<tables::HashedAccount>().unwrap().len(), 5);

        // second run, hash next five accounts.
        input.checkpoint = Some(result.unwrap().checkpoint);
        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number: 20,
                    stage_checkpoint: Some(StageUnitCheckpoint::Account(
                        AccountHashingCheckpoint {
                            address: None,
                            block_range: CheckpointBlockRange {
                                from: 0,
                                to: 0,
                            },
                            progress: EntitiesCheckpoint { processed, total }
                        }
                    ))
                },
                done: true
            }) if processed == total &&
                total == runner.tx.table::<tables::PlainAccountState>().unwrap().len() as u64
        );
        assert_eq!(runner.tx.table::<tables::HashedAccount>().unwrap().len(), 10);

        // Validate the stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    mod test_utils {
        use super::*;
        use crate::{
            stages::hashing_account::AccountHashingStage,
            test_utils::{StageTestRunner, TestTransaction},
            ExecInput, ExecOutput, UnwindInput,
        };
        use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};
        use reth_primitives::Address;

        pub(crate) struct AccountHashingTestRunner {
            pub(crate) tx: TestTransaction,
            commit_threshold: u64,
            clean_threshold: u64,
        }

        impl AccountHashingTestRunner {
            pub(crate) fn set_clean_threshold(&mut self, threshold: u64) {
                self.clean_threshold = threshold;
            }

            #[allow(dead_code)]
            pub(crate) fn set_commit_threshold(&mut self, threshold: u64) {
                self.commit_threshold = threshold;
            }

            /// Iterates over PlainAccount table and checks that the accounts match the ones
            /// in the HashedAccount table
            pub(crate) fn check_hashed_accounts(&self) -> Result<(), TestRunnerError> {
                self.tx.query(|tx| {
                    let mut acc_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut hashed_acc_cursor = tx.cursor_read::<tables::HashedAccount>()?;

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

            /// Same as check_hashed_accounts, only that checks with the old account state,
            /// namely, the same account with nonce - 1 and balance - 1.
            pub(crate) fn check_old_hashed_accounts(&self) -> Result<(), TestRunnerError> {
                self.tx.query(|tx| {
                    let mut acc_cursor = tx.cursor_read::<tables::PlainAccountState>()?;
                    let mut hashed_acc_cursor = tx.cursor_read::<tables::HashedAccount>()?;

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
                    tx: TestTransaction::default(),
                    commit_threshold: 1000,
                    clean_threshold: 1000,
                }
            }
        }

        impl StageTestRunner for AccountHashingTestRunner {
            type S = AccountHashingStage;

            fn tx(&self) -> &TestTransaction {
                &self.tx
            }

            fn stage(&self) -> Self::S {
                Self::S {
                    commit_threshold: self.commit_threshold,
                    clean_threshold: self.clean_threshold,
                }
            }
        }

        #[async_trait::async_trait]
        impl ExecuteStageTestRunner for AccountHashingTestRunner {
            type Seed = Vec<(Address, Account)>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                Ok(AccountHashingStage::seed(
                    &mut self.tx.inner(),
                    SeedOpts {
                        blocks: 1..=input.previous_stage_checkpoint_block_number(),
                        accounts: 0..10,
                        txs: 0..3,
                    },
                )
                .unwrap())
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
