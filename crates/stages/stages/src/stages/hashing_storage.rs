use alloy_primitives::{b256, bytes::BufMut, keccak256, Address, B256};
use itertools::Itertools;
use reth_config::config::{EtlConfig, HashingConfig};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRW},
    models::CompactU256,
    table::Decompress,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_etl::Collector;
use reth_primitives_traits::StorageEntry;
use reth_provider::{DBProvider, HashingWriter, StatsReader, StorageReader};
use reth_stages_api::{
    EntitiesCheckpoint, ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId,
    StorageHashingCheckpoint, UnwindInput, UnwindOutput,
};
use reth_storage_errors::provider::ProviderResult;
use std::{
    fmt::Debug,
    sync::mpsc::{self, Receiver},
};
use tracing::*;

/// Maximum number of channels that can exist in memory.
const MAXIMUM_CHANNELS: usize = 10_000;

/// Maximum number of storage entries to hash per rayon worker job.
const WORKER_CHUNK_SIZE: usize = 100;

/// Keccak256 hash of the zero address.
const HASHED_ZERO_ADDRESS: B256 =
    b256!("0x5380c7b7ae81a58eb98d9c78de4a1fd7fd9535fc953ed2be602daaa41767312a");

/// Storage hashing stage hashes plain storage.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
#[derive(Debug)]
pub struct StorageHashingStage {
    /// The threshold (in number of blocks) for switching between incremental
    /// hashing and full storage hashing.
    pub clean_threshold: u64,
    /// The maximum number of slots to process before committing during unwind.
    pub commit_threshold: u64,
    /// ETL configuration
    pub etl_config: EtlConfig,
}

impl StorageHashingStage {
    /// Create new instance of [`StorageHashingStage`].
    pub const fn new(config: HashingConfig, etl_config: EtlConfig) -> Self {
        Self {
            clean_threshold: config.clean_threshold,
            commit_threshold: config.commit_threshold,
            etl_config,
        }
    }
}

impl Default for StorageHashingStage {
    fn default() -> Self {
        Self {
            clean_threshold: 500_000,
            commit_threshold: 100_000,
            etl_config: EtlConfig::default(),
        }
    }
}

impl<Provider> Stage<Provider> for StorageHashingStage
where
    Provider: DBProvider<Tx: DbTxMut> + StorageReader + HashingWriter + StatsReader,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::StorageHashing
    }

    /// Execute the stage.
    ///
    /// NOTE: This stage is now a no-op because the execution stage writes directly to
    /// HashedStorages. This stage exists only for backwards compatibility with existing pipelines.
    fn execute(
        &mut self,
        _provider: &Provider,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        // Since execution now writes directly to HashedStorages, this stage is a no-op.
        // Just report that we're done up to the target block.
        Ok(ExecOutput::done(input.checkpoint().with_block_number(input.target())))
    }

    /// Unwind the stage.
    ///
    /// NOTE: This stage is now a no-op because the execution stage manages HashedStorages directly.
    fn unwind(
        &mut self,
        _provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // Since execution now manages HashedStorages directly during unwind, this is a no-op.
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

/// Flushes channels hashes to ETL collector.
fn collect(
    channels: &mut Vec<Receiver<(Vec<u8>, CompactU256)>>,
    collector: &mut Collector<Vec<u8>, CompactU256>,
) -> Result<(), StageError> {
    for channel in channels.iter_mut() {
        while let Ok((key, v)) = channel.recv() {
            collector.insert(key, v)?;
        }
    }
    info!(target: "sync::stages::hashing_storage", "Hashed {} entries", collector.len());
    channels.clear();
    Ok(())
}

fn stage_checkpoint_progress(provider: &impl StatsReader) -> ProviderResult<EntitiesCheckpoint> {
    Ok(EntitiesCheckpoint {
        processed: provider.count_entries::<tables::HashedStorages>()? as u64,
        total: provider.count_entries::<tables::PlainStorageState>()? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestStageDB, UnwindStageTestRunner,
    };
    use alloy_primitives::{Address, U256};
    use assert_matches::assert_matches;
    use rand::Rng;
    use reth_db_api::{
        cursor::{DbCursorRW, DbDupCursorRO},
        models::{BlockNumberAddress, StoredBlockBodyIndices},
    };
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::SealedBlock;
    use reth_provider::providers::StaticFileWriter;
    use reth_testing_utils::generators::{
        self, random_block_range, random_contract_account_range, BlockRangeParams,
    };

    stage_test_suite_ext!(StorageHashingTestRunner, storage_hashing);

    /// Execute with low clean threshold so as to hash whole storage
    #[tokio::test]
    async fn execute_clean_storage_hashing() {
        let (previous_stage, stage_progress) = (500, 100);

        // Set up the runner
        let mut runner = StorageHashingTestRunner::default();

        // set low clean threshold so we hash the whole storage
        runner.set_clean_threshold(1);

        // set low commit threshold so we force each entry to be a tx.commit and make sure we don't
        // hang on one key. Seed execution inserts more than one storage entry per address.
        runner.set_commit_threshold(1);

        let mut input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        loop {
            if let Ok(result @ ExecOutput { checkpoint, done }) =
                runner.execute(input).await.unwrap()
            {
                if !done {
                    let previous_checkpoint = input
                        .checkpoint
                        .and_then(|checkpoint| checkpoint.storage_hashing_stage_checkpoint())
                        .unwrap_or_default();
                    assert_matches!(checkpoint.storage_hashing_stage_checkpoint(), Some(StorageHashingCheckpoint {
                        progress: EntitiesCheckpoint {
                            processed,
                            total,
                        },
                        ..
                    }) if processed == previous_checkpoint.progress.processed + 1 &&
                        total == runner.db.count_entries::<tables::PlainStorageState>().unwrap() as u64);

                    // Continue from checkpoint
                    input.checkpoint = Some(checkpoint);
                    continue
                }
                assert_eq!(checkpoint.block_number, previous_stage);
                assert_matches!(checkpoint.storage_hashing_stage_checkpoint(), Some(StorageHashingCheckpoint {
                        progress: EntitiesCheckpoint {
                            processed,
                            total,
                        },
                        ..
                    }) if processed == total &&
                        total == runner.db.count_entries::<tables::PlainStorageState>().unwrap() as u64);

                // Validate the stage execution
                assert!(
                    runner.validate_execution(input, Some(result)).is_ok(),
                    "execution validation"
                );

                break
            }
            panic!("Failed execution");
        }
    }

    struct StorageHashingTestRunner {
        db: TestStageDB,
        commit_threshold: u64,
        clean_threshold: u64,
        etl_config: EtlConfig,
    }

    impl Default for StorageHashingTestRunner {
        fn default() -> Self {
            Self {
                db: TestStageDB::default(),
                commit_threshold: 1000,
                clean_threshold: 1000,
                etl_config: EtlConfig::default(),
            }
        }
    }

    impl StageTestRunner for StorageHashingTestRunner {
        type S = StorageHashingStage;

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

    impl ExecuteStageTestRunner for StorageHashingTestRunner {
        type Seed = Vec<SealedBlock<Block>>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.next_block();
            let end = input.target();
            let mut rng = generators::rng();

            let n_accounts = 31;
            let mut accounts = random_contract_account_range(&mut rng, &mut (0..n_accounts));

            let blocks = random_block_range(
                &mut rng,
                stage_progress..=end,
                BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..3, ..Default::default() },
            );

            self.db.insert_headers(blocks.iter().map(|block| block.sealed_header()))?;
            let mut tx_hash_numbers = Vec::new();

            let iter = blocks.iter();
            let mut next_tx_num = 0;
            let mut first_tx_num = next_tx_num;
            for progress in iter {
                // Insert last progress data
                let block_number = progress.number;
                self.db.commit(|tx| {
                    progress.body().transactions.iter().try_for_each(
                        |transaction| -> Result<(), reth_db::DatabaseError> {
                            tx_hash_numbers.push((*transaction.tx_hash(), next_tx_num));
                            tx.put::<tables::Transactions>(next_tx_num, transaction.clone())?;

                            let (addr, _) = accounts
                                .get_mut((rng.random::<u64>() % n_accounts) as usize)
                                .unwrap();

                            for _ in 0..2 {
                                let new_entry = StorageEntry {
                                    key: keccak256([rng.random::<u8>()]),
                                    value: U256::from(rng.random::<u8>() % 30 + 1),
                                };
                                self.insert_storage_entry(
                                    tx,
                                    (block_number, *addr).into(),
                                    new_entry,
                                    progress.number == stage_progress,
                                )?;
                            }

                            next_tx_num += 1;
                            Ok(())
                        },
                    )?;

                    // Randomize rewards
                    let has_reward: bool = rng.random();
                    if has_reward {
                        self.insert_storage_entry(
                            tx,
                            (block_number, Address::random()).into(),
                            StorageEntry {
                                key: keccak256("mining"),
                                value: U256::from(rng.random::<u32>()),
                            },
                            progress.number == stage_progress,
                        )?;
                    }

                    let body = StoredBlockBodyIndices {
                        first_tx_num,
                        tx_count: progress.transaction_count() as u64,
                    };

                    first_tx_num = next_tx_num;

                    tx.put::<tables::BlockBodyIndices>(progress.number, body)?;
                    Ok(())
                })?;
            }
            self.db.insert_tx_hash_numbers(tx_hash_numbers)?;

            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                let start_block = input.checkpoint().block_number + 1;
                let end_block = output.checkpoint.block_number;
                if start_block > end_block {
                    return Ok(())
                }
            }
            self.check_hashed_storage()
        }
    }

    impl UnwindStageTestRunner for StorageHashingTestRunner {
        fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            self.unwind_storage(input)?;
            self.check_hashed_storage()
        }
    }

    impl StorageHashingTestRunner {
        fn set_clean_threshold(&mut self, threshold: u64) {
            self.clean_threshold = threshold;
        }

        fn set_commit_threshold(&mut self, threshold: u64) {
            self.commit_threshold = threshold;
        }

        fn check_hashed_storage(&self) -> Result<(), TestRunnerError> {
            self.db
                .query(|tx| {
                    let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>()?;
                    let mut hashed_storage_cursor =
                        tx.cursor_dup_read::<tables::HashedStorages>()?;

                    let mut expected = 0;

                    while let Some((address, entry)) = storage_cursor.next()? {
                        let key = keccak256(entry.key);
                        let got =
                            hashed_storage_cursor.seek_by_key_subkey(keccak256(address), key)?;
                        assert_eq!(
                            got,
                            Some(StorageEntry { key, ..entry }),
                            "{expected}: {address:?}"
                        );
                        expected += 1;
                    }
                    let count = tx.cursor_dup_read::<tables::HashedStorages>()?.walk(None)?.count();

                    assert_eq!(count, expected);
                    Ok(())
                })
                .map_err(|e| e.into())
        }

        fn insert_storage_entry<TX: DbTxMut>(
            &self,
            tx: &TX,
            bn_address: BlockNumberAddress,
            entry: StorageEntry,
            hash: bool,
        ) -> Result<(), reth_db::DatabaseError> {
            let mut storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
            let prev_entry =
                match storage_cursor.seek_by_key_subkey(bn_address.address(), entry.key)? {
                    Some(e) if e.key == entry.key => {
                        tx.delete::<tables::PlainStorageState>(bn_address.address(), Some(e))
                            .expect("failed to delete entry");
                        e
                    }
                    _ => StorageEntry { key: entry.key, value: U256::from(0) },
                };
            tx.put::<tables::PlainStorageState>(bn_address.address(), entry)?;

            if hash {
                let hashed_address = keccak256(bn_address.address());
                let hashed_entry = StorageEntry { key: keccak256(entry.key), value: entry.value };

                if let Some(e) = tx
                    .cursor_dup_write::<tables::HashedStorages>()?
                    .seek_by_key_subkey(hashed_address, hashed_entry.key)?
                    .filter(|e| e.key == hashed_entry.key)
                {
                    tx.delete::<tables::HashedStorages>(hashed_address, Some(e))
                        .expect("failed to delete entry");
                }

                tx.put::<tables::HashedStorages>(hashed_address, hashed_entry)?;
            }

            tx.put::<tables::StorageChangeSets>(bn_address, prev_entry)?;
            Ok(())
        }

        fn unwind_storage(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            tracing::debug!("unwinding storage...");
            let target_block = input.unwind_to;
            self.db.commit(|tx| {
                let mut storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                let mut changeset_cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;

                let mut rev_changeset_walker = changeset_cursor.walk_back(None)?;

                while let Some((bn_address, entry)) = rev_changeset_walker.next().transpose()? {
                    if bn_address.block_number() < target_block {
                        break
                    }

                    if storage_cursor
                        .seek_by_key_subkey(bn_address.address(), entry.key)?
                        .filter(|e| e.key == entry.key)
                        .is_some()
                    {
                        storage_cursor.delete_current()?;
                    }

                    if !entry.value.is_zero() {
                        storage_cursor.upsert(bn_address.address(), &entry)?;
                    }
                }
                Ok(())
            })?;
            Ok(())
        }
    }
}
