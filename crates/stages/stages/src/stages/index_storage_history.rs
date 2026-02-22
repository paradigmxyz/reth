use super::{collect_history_indices, collect_storage_history_indices};
use crate::{stages::utils::load_storage_history, StageCheckpoint, StageId};
use reth_config::config::{EtlConfig, IndexHistoryConfig};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_db_api::Tables;
use reth_db_api::{
    models::{storage_sharded_key::StorageShardedKey, AddressStorageKey, BlockNumberAddress},
    tables,
    transaction::DbTxMut,
};
use reth_provider::{
    DBProvider, EitherWriter, HistoryWriter, PruneCheckpointReader, PruneCheckpointWriter,
    RocksDBProviderFactory, StaticFileProviderFactory, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment};
use reth_stages_api::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use std::fmt::Debug;
use tracing::info;

/// Stage is indexing history the storage changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`tables::StoragesHistory`].
#[derive(Debug)]
pub struct IndexStorageHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
    /// Pruning configuration.
    pub prune_mode: Option<PruneMode>,
    /// ETL configuration
    pub etl_config: EtlConfig,
    /// Whether to emit progress/info logs while collecting/writing indices.
    pub log_progress: bool,
}

impl IndexStorageHistoryStage {
    /// Create new instance of [`IndexStorageHistoryStage`].
    pub const fn new(
        config: IndexHistoryConfig,
        etl_config: EtlConfig,
        prune_mode: Option<PruneMode>,
    ) -> Self {
        Self {
            commit_threshold: config.commit_threshold,
            etl_config,
            prune_mode,
            log_progress: true,
        }
    }

    /// Enables or disables progress/info logging for this stage execution.
    pub const fn with_progress_logging(mut self, log_progress: bool) -> Self {
        self.log_progress = log_progress;
        self
    }
}

impl Default for IndexStorageHistoryStage {
    fn default() -> Self {
        Self {
            commit_threshold: 100_000,
            prune_mode: None,
            etl_config: EtlConfig::default(),
            log_progress: true,
        }
    }
}

impl<Provider> Stage<Provider> for IndexStorageHistoryStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + StorageSettingsCache
        + RocksDBProviderFactory
        + StorageChangeSetReader
        + StaticFileProviderFactory
        + reth_provider::NodePrimitivesProvider,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexStorageHistory
    }

    /// Execute the stage.
    fn execute(
        &mut self,
        provider: &Provider,
        mut input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if let Some((target_prunable_block, prune_mode)) = self
            .prune_mode
            .map(|mode| {
                mode.prune_target_block(
                    input.target(),
                    PruneSegment::StorageHistory,
                    PrunePurpose::User,
                )
            })
            .transpose()?
            .flatten() &&
            target_prunable_block > input.checkpoint().block_number
        {
            input.checkpoint = Some(StageCheckpoint::new(target_prunable_block));

            // Save prune checkpoint only if we don't have one already.
            // Otherwise, pruner may skip the unpruned range of blocks.
            if provider.get_prune_checkpoint(PruneSegment::StorageHistory)?.is_none() {
                provider.save_prune_checkpoint(
                    PruneSegment::StorageHistory,
                    PruneCheckpoint {
                        block_number: Some(target_prunable_block),
                        tx_number: None,
                        prune_mode,
                    },
                )?;
            }
        }

        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let mut range = input.next_block_range();
        let first_sync = input.checkpoint().block_number == 0;
        let use_rocksdb = provider.cached_storage_settings().storage_v2;

        // On first sync we might have history coming from genesis. We clear the table since it's
        // faster to rebuild from scratch.
        if first_sync {
            if use_rocksdb {
                // Note: RocksDB clear() executes immediately (not deferred to commit like MDBX),
                // but this is safe for first_sync because if we crash before commit, the
                // checkpoint stays at 0 and we'll just clear and rebuild again on restart. The
                // source data (changesets) is intact.
                provider.rocksdb_provider().clear::<tables::StoragesHistory>()?;
            } else {
                provider.tx_ref().clear::<tables::StoragesHistory>()?;
            }
            range = 0..=*input.next_block_range().end();
        }

        if self.log_progress {
            info!(target: "sync::stages::index_storage_history::exec", ?first_sync, ?use_rocksdb, "Collecting indices");
        }
        let collector = if provider.cached_storage_settings().storage_v2 {
            collect_storage_history_indices(
                provider,
                range.clone(),
                &self.etl_config,
                self.log_progress,
            )?
        } else {
            collect_history_indices::<_, tables::StorageChangeSets, tables::StoragesHistory, _>(
                provider,
                BlockNumberAddress::range(range.clone()),
                |AddressStorageKey((address, storage_key)), highest_block_number| {
                    StorageShardedKey::new(address, storage_key, highest_block_number)
                },
                |(key, value)| (key.block_number(), AddressStorageKey((key.address(), value.key))),
                &self.etl_config,
                self.log_progress,
            )?
        };

        if self.log_progress {
            info!(target: "sync::stages::index_storage_history::exec", "Loading indices into database");
        }

        provider.with_rocksdb_batch_auto_commit(|rocksdb_batch| {
            let mut writer = EitherWriter::new_storages_history(provider, rocksdb_batch)?;
            load_storage_history(collector, first_sync, &mut writer, self.log_progress)
                .map_err(|e| reth_provider::ProviderError::other(Box::new(e)))?;
            Ok(((), writer.into_raw_rocksdb_batch()))
        })?;

        #[cfg(all(unix, feature = "rocksdb"))]
        if use_rocksdb {
            provider.commit_pending_rocksdb_batches()?;
            provider.rocksdb_provider().flush(&[Tables::StoragesHistory.name()])?;
        }

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(*range.end()), done: true })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        provider.unwind_storage_history_indices_range(range)?;

        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(unwind_progress) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestStageDB, UnwindStageTestRunner,
    };
    use alloy_primitives::{address, b256, Address, BlockNumber, B256, U256};
    use itertools::Itertools;
    use reth_db_api::{
        cursor::DbCursorRO,
        models::{
            sharded_key, storage_sharded_key::NUM_OF_INDICES_IN_SHARD, ShardedKey,
            StoredBlockBodyIndices,
        },
        transaction::DbTx,
        BlockNumberList,
    };
    use reth_primitives_traits::StorageEntry;
    use reth_provider::{providers::StaticFileWriter, DatabaseProviderFactory};
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_contract_account_range,
        BlockRangeParams,
    };
    use std::collections::BTreeMap;

    const ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");
    const STORAGE_KEY: B256 =
        b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

    const LAST_BLOCK_IN_FULL_SHARD: BlockNumber = NUM_OF_INDICES_IN_SHARD as BlockNumber;
    const MAX_BLOCK: BlockNumber = NUM_OF_INDICES_IN_SHARD as BlockNumber + 2;

    const fn storage(key: B256) -> StorageEntry {
        // Value is not used in indexing stage.
        StorageEntry { key, value: U256::ZERO }
    }

    const fn block_number_address(block_number: u64) -> BlockNumberAddress {
        BlockNumberAddress((block_number, ADDRESS))
    }

    /// Shard for account
    const fn shard(shard_index: u64) -> StorageShardedKey {
        StorageShardedKey {
            address: ADDRESS,
            sharded_key: ShardedKey { key: STORAGE_KEY, highest_block_number: shard_index },
        }
    }

    fn list(list: &[u64]) -> BlockNumberList {
        BlockNumberList::new(list.iter().copied()).unwrap()
    }

    fn cast(
        table: Vec<(StorageShardedKey, BlockNumberList)>,
    ) -> BTreeMap<StorageShardedKey, Vec<u64>> {
        table
            .into_iter()
            .map(|(k, v)| {
                let v = v.iter().collect();
                (k, v)
            })
            .collect()
    }

    fn partial_setup(db: &TestStageDB) {
        // setup
        db.commit(|tx| {
            for block in 0..=MAX_BLOCK {
                tx.put::<tables::BlockBodyIndices>(
                    block,
                    StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                )?;
                // setup changeset that is going to be applied to history index
                tx.put::<tables::StorageChangeSets>(
                    block_number_address(block),
                    storage(STORAGE_KEY),
                )?;
            }
            Ok(())
        })
        .unwrap()
    }

    fn run(db: &TestStageDB, run_to: u64, input_checkpoint: Option<BlockNumber>) {
        let input = ExecInput {
            target: Some(run_to),
            checkpoint: input_checkpoint
                .map(|block_number| StageCheckpoint { block_number, stage_checkpoint: None }),
        };
        let mut stage = IndexStorageHistoryStage::default();
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(run_to), done: true });
        provider.commit().unwrap();
    }

    fn unwind(db: &TestStageDB, unwind_from: u64, unwind_to: u64) {
        let input = UnwindInput {
            checkpoint: StageCheckpoint::new(unwind_from),
            unwind_to,
            ..Default::default()
        };
        let mut stage = IndexStorageHistoryStage::default();
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.unwind(&provider, input).unwrap();
        assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) });
        provider.commit().unwrap();
    }

    #[tokio::test]
    async fn insert_index_to_genesis() {
        // init
        let db = TestStageDB::default();

        // setup
        partial_setup(&db);

        // run
        run(&db, 3, None);

        // verify
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![0, 1, 2, 3])]));

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![0])]));
    }

    #[tokio::test]
    async fn insert_index_to_not_empty_shard() {
        // init
        let db = TestStageDB::default();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StoragesHistory>(shard(u64::MAX), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, 5, Some(3));

        // verify
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 5])]));

        // unwind
        unwind(&db, 5, 3);

        // verify initial state
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3])]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let db = TestStageDB::default();
        // change does not matter only that account is present in changeset.
        let full_list = (1..=LAST_BLOCK_IN_FULL_SHARD).collect::<Vec<_>>();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StoragesHistory>(shard(u64::MAX), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD + 2, Some(LAST_BLOCK_IN_FULL_SHARD));

        // verify
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(LAST_BLOCK_IN_FULL_SHARD), full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1, LAST_BLOCK_IN_FULL_SHARD + 2])
            ])
        );

        // unwind
        unwind(&db, LAST_BLOCK_IN_FULL_SHARD + 2, LAST_BLOCK_IN_FULL_SHARD);

        // verify initial state
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), full_list)]));
    }

    #[tokio::test]
    async fn insert_index_to_fill_shard() {
        // init
        let db = TestStageDB::default();
        let mut almost_full_list = (1..=LAST_BLOCK_IN_FULL_SHARD - 2).collect::<Vec<_>>();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StoragesHistory>(shard(u64::MAX), list(&almost_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD, Some(LAST_BLOCK_IN_FULL_SHARD - 2));

        // verify
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD - 1);
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list.clone())]));

        // unwind
        unwind(&db, LAST_BLOCK_IN_FULL_SHARD, LAST_BLOCK_IN_FULL_SHARD - 2);

        // verify initial state
        almost_full_list.pop();
        almost_full_list.pop();
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list)]));

        // verify initial state
    }

    #[tokio::test]
    async fn insert_index_second_half_shard() {
        // init
        let db = TestStageDB::default();
        let mut almost_full_list = (1..=LAST_BLOCK_IN_FULL_SHARD - 1).collect::<Vec<_>>();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StoragesHistory>(shard(u64::MAX), list(&almost_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD + 1, Some(LAST_BLOCK_IN_FULL_SHARD - 1));

        // verify
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(LAST_BLOCK_IN_FULL_SHARD), almost_full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1])
            ])
        );

        // unwind
        unwind(&db, LAST_BLOCK_IN_FULL_SHARD, LAST_BLOCK_IN_FULL_SHARD - 1);

        // verify initial state
        almost_full_list.pop();
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list)]));
    }

    #[tokio::test]
    async fn insert_index_to_third_shard() {
        // init
        let db = TestStageDB::default();
        let full_list = (1..=LAST_BLOCK_IN_FULL_SHARD).collect::<Vec<_>>();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StoragesHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::StoragesHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::StoragesHistory>(
                shard(u64::MAX),
                list(&[LAST_BLOCK_IN_FULL_SHARD + 1]),
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        run(&db, LAST_BLOCK_IN_FULL_SHARD + 2, Some(LAST_BLOCK_IN_FULL_SHARD + 1));

        // verify
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1, LAST_BLOCK_IN_FULL_SHARD + 2])
            ])
        );

        // unwind
        unwind(&db, LAST_BLOCK_IN_FULL_SHARD + 2, LAST_BLOCK_IN_FULL_SHARD + 1);

        // verify initial state
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1])
            ])
        );
    }

    #[tokio::test]
    async fn insert_index_with_prune_mode() {
        // init
        let db = TestStageDB::default();

        // setup
        db.commit(|tx| {
            // we just need first and last
            tx.put::<tables::BlockBodyIndices>(
                0,
                StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
            )
            .unwrap();

            tx.put::<tables::BlockBodyIndices>(
                100,
                StoredBlockBodyIndices { tx_count: 5, ..Default::default() },
            )
            .unwrap();

            // setup changeset that are going to be applied to history index
            tx.put::<tables::StorageChangeSets>(block_number_address(20), storage(STORAGE_KEY))
                .unwrap();
            tx.put::<tables::StorageChangeSets>(block_number_address(36), storage(STORAGE_KEY))
                .unwrap();
            tx.put::<tables::StorageChangeSets>(block_number_address(100), storage(STORAGE_KEY))
                .unwrap();
            Ok(())
        })
        .unwrap();

        // run
        let input = ExecInput { target: Some(20000), ..Default::default() };
        let mut stage = IndexStorageHistoryStage {
            prune_mode: Some(PruneMode::Before(36)),
            ..Default::default()
        };
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(20000), done: true });
        provider.commit().unwrap();

        // verify
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![36, 100])]));

        // unwind
        unwind(&db, 20000, 0);

        // verify initial state
        let table = db.table::<tables::StoragesHistory>().unwrap();
        assert!(table.is_empty());
    }

    stage_test_suite_ext!(IndexStorageHistoryTestRunner, index_storage_history);

    struct IndexStorageHistoryTestRunner {
        pub(crate) db: TestStageDB,
        commit_threshold: u64,
        prune_mode: Option<PruneMode>,
    }

    impl Default for IndexStorageHistoryTestRunner {
        fn default() -> Self {
            Self { db: TestStageDB::default(), commit_threshold: 1000, prune_mode: None }
        }
    }

    impl StageTestRunner for IndexStorageHistoryTestRunner {
        type S = IndexStorageHistoryStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            Self::S {
                commit_threshold: self.commit_threshold,
                prune_mode: self.prune_mode,
                etl_config: EtlConfig::default(),
                log_progress: true,
            }
        }
    }

    impl ExecuteStageTestRunner for IndexStorageHistoryTestRunner {
        type Seed = ();

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_process = input.checkpoint().block_number;
            let start = stage_process + 1;
            let end = input.target();
            let mut rng = generators::rng();

            let num_of_accounts = 31;
            let accounts = random_contract_account_range(&mut rng, &mut (0..num_of_accounts))
                .into_iter()
                .collect::<BTreeMap<_, _>>();

            let blocks = random_block_range(
                &mut rng,
                start..=end,
                BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..3, ..Default::default() },
            );

            let (changesets, _) = random_changeset_range(
                &mut rng,
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..u64::MAX,
            );

            // add block changeset from block 1.
            self.db.insert_changesets(changesets, Some(start))?;

            Ok(())
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

                assert_eq!(
                    output,
                    ExecOutput { checkpoint: StageCheckpoint::new(input.target()), done: true }
                );

                let provider = self.db.factory.provider()?;
                let mut changeset_cursor =
                    provider.tx_ref().cursor_read::<tables::StorageChangeSets>()?;

                let storage_transitions = changeset_cursor
                    .walk_range(BlockNumberAddress::range(start_block..=end_block))?
                    .try_fold(
                        BTreeMap::new(),
                        |mut storages: BTreeMap<(Address, B256), Vec<u64>>,
                         entry|
                         -> Result<_, TestRunnerError> {
                            let (index, storage) = entry?;
                            storages
                                .entry((index.address(), storage.key))
                                .or_default()
                                .push(index.block_number());
                            Ok(storages)
                        },
                    )?;

                let mut result = BTreeMap::new();
                for (partial_key, indices) in storage_transitions {
                    // chunk indices and insert them in shards of N size.
                    let mut chunks = indices
                        .iter()
                        .chunks(sharded_key::NUM_OF_INDICES_IN_SHARD)
                        .into_iter()
                        .map(|chunks| chunks.copied().collect::<Vec<u64>>())
                        .collect::<Vec<Vec<_>>>();
                    let last_chunk = chunks.pop();

                    for list in chunks {
                        result.insert(
                            StorageShardedKey::new(
                                partial_key.0,
                                partial_key.1,
                                *list.last().expect("Chuck does not return empty list")
                                    as BlockNumber,
                            ),
                            list,
                        );
                    }

                    if let Some(last_list) = last_chunk {
                        result.insert(
                            StorageShardedKey::new(partial_key.0, partial_key.1, u64::MAX),
                            last_list,
                        );
                    };
                }

                let table = cast(self.db.table::<tables::StoragesHistory>().unwrap());
                assert_eq!(table, result);
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for IndexStorageHistoryTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            let table = self.db.table::<tables::StoragesHistory>().unwrap();
            assert!(table.is_empty());
            Ok(())
        }
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    mod rocksdb_tests {
        use super::*;
        use reth_db_api::models::StorageBeforeTx;
        use reth_provider::{providers::StaticFileWriter, RocksDBProviderFactory};
        use reth_static_file_types::StaticFileSegment;
        use reth_storage_api::StorageSettings;

        /// Sets up v2 storage test data: writes block body indices to MDBX and
        /// storage changesets to static files (matching realistic v2 layout).
        fn setup_v2_storage_data(db: &TestStageDB, block_range: std::ops::RangeInclusive<u64>) {
            db.factory.set_storage_settings_cache(StorageSettings::v2());

            db.commit(|tx| {
                for block in block_range.clone() {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                }
                Ok(())
            })
            .unwrap();

            let static_file_provider = db.factory.static_file_provider();
            let mut writer =
                static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();
            for block in block_range {
                writer
                    .append_storage_changeset(
                        vec![StorageBeforeTx {
                            address: ADDRESS,
                            key: STORAGE_KEY,
                            value: U256::ZERO,
                        }],
                        block,
                    )
                    .unwrap();
            }
            writer.commit().unwrap();
        }

        /// Test that when `storages_history_in_rocksdb` is enabled, the stage
        /// writes storage history indices to `RocksDB` instead of MDBX.
        #[tokio::test]
        async fn execute_writes_to_rocksdb_when_enabled() {
            let db = TestStageDB::default();
            setup_v2_storage_data(&db, 0..=10);

            let input = ExecInput { target: Some(10), ..Default::default() };
            let mut stage = IndexStorageHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
            provider.commit().unwrap();

            let mdbx_table = db.table::<tables::StoragesHistory>().unwrap();
            assert!(
                mdbx_table.is_empty(),
                "MDBX StoragesHistory should be empty when RocksDB is enabled"
            );

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should contain storage history");

            let block_list = result.unwrap();
            let blocks: Vec<u64> = block_list.iter().collect();
            assert_eq!(blocks, (0..=10).collect::<Vec<_>>());
        }

        /// Test that unwind works correctly when `storages_history_in_rocksdb` is enabled.
        #[tokio::test]
        async fn unwind_works_when_rocksdb_enabled() {
            let db = TestStageDB::default();
            setup_v2_storage_data(&db, 0..=10);

            let input = ExecInput { target: Some(10), ..Default::default() };
            let mut stage = IndexStorageHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should have data before unwind");
            let blocks_before: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks_before, (0..=10).collect::<Vec<_>>());

            let unwind_input =
                UnwindInput { checkpoint: StageCheckpoint::new(10), unwind_to: 5, bad_block: None };
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.unwind(&provider, unwind_input).unwrap();
            assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(5) });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should still have data after partial unwind");
            let blocks_after: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(
                blocks_after,
                (0..=5).collect::<Vec<_>>(),
                "Should only have blocks 0-5 after unwind to block 5"
            );
        }

        /// Test that unwind to block 0 keeps only block 0's history.
        #[tokio::test]
        async fn unwind_to_zero_keeps_block_zero() {
            let db = TestStageDB::default();
            setup_v2_storage_data(&db, 0..=5);

            let input = ExecInput { target: Some(5), ..Default::default() };
            let mut stage = IndexStorageHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(5), done: true });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should have data before unwind");

            let unwind_input =
                UnwindInput { checkpoint: StageCheckpoint::new(5), unwind_to: 0, bad_block: None };
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.unwind(&provider, unwind_input).unwrap();
            assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(0) });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should still have block 0 history");
            let blocks_after: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks_after, vec![0], "Should only have block 0 after unwinding to 0");
        }

        /// Test incremental sync merges new data with existing shards.
        #[tokio::test]
        async fn execute_incremental_sync() {
            let db = TestStageDB::default();
            setup_v2_storage_data(&db, 0..=10);

            let input = ExecInput { target: Some(5), ..Default::default() };
            let mut stage = IndexStorageHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(5), done: true });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some());
            let blocks: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks, (0..=5).collect::<Vec<_>>());

            let input = ExecInput { target: Some(10), checkpoint: Some(StageCheckpoint::new(5)) };
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should have merged data");
            let blocks: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks, (0..=10).collect::<Vec<_>>());
        }

        /// Test stale checkpoint overlap currently panics due non-monotonic shard merge input.
        ///
        /// This models restart/crash recovery where `RocksDB` writes can be ahead of stage
        /// checkpoint. Re-executing from stale checkpoint replays overlap and hits the sorted-list
        /// invariant panic in shard encoding.
        #[tokio::test]
        #[should_panic(expected = "IntegerList must be pre-sorted and non-empty")]
        async fn execute_incremental_sync_with_stale_checkpoint_overlap_panics() {
            let db = TestStageDB::default();
            setup_v2_storage_data(&db, 0..=10);

            let input = ExecInput { target: Some(5), ..Default::default() };
            let mut stage = IndexStorageHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(5), done: true });
            provider.commit().unwrap();

            // Simulate a stale checkpoint (3) while RocksDB already contains data up to 5.
            let input = ExecInput { target: Some(10), checkpoint: Some(StageCheckpoint::new(3)) };
            let provider = db.factory.database_provider_rw().unwrap();
            let _ = stage.execute(&provider, input).unwrap();
        }

        /// Test multi-shard unwind correctly handles shards that span across unwind boundary.
        #[tokio::test]
        async fn unwind_multi_shard() {
            use reth_db_api::models::sharded_key::NUM_OF_INDICES_IN_SHARD;

            let db = TestStageDB::default();
            let num_blocks = (NUM_OF_INDICES_IN_SHARD * 2 + 100) as u64;
            setup_v2_storage_data(&db, 0..=num_blocks - 1);

            let input = ExecInput { target: Some(num_blocks - 1), ..Default::default() };
            let mut stage = IndexStorageHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(
                out,
                ExecOutput { checkpoint: StageCheckpoint::new(num_blocks - 1), done: true }
            );
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let shards = rocksdb.storage_history_shards(ADDRESS, STORAGE_KEY).unwrap();
            assert!(shards.len() >= 2, "Should have at least 2 shards for {} blocks", num_blocks);

            let unwind_to = NUM_OF_INDICES_IN_SHARD as u64 + 50;
            let unwind_input = UnwindInput {
                checkpoint: StageCheckpoint::new(num_blocks - 1),
                unwind_to,
                bad_block: None,
            };
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.unwind(&provider, unwind_input).unwrap();
            assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let shards_after = rocksdb.storage_history_shards(ADDRESS, STORAGE_KEY).unwrap();
            assert!(!shards_after.is_empty(), "Should still have shards after unwind");

            let all_blocks: Vec<u64> =
                shards_after.iter().flat_map(|(_, list)| list.iter()).collect();
            assert_eq!(
                all_blocks,
                (0..=unwind_to).collect::<Vec<_>>(),
                "Should only have blocks 0 to {} after unwind",
                unwind_to
            );
        }
    }
}
