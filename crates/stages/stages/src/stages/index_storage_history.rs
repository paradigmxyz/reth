use super::{collect_history_indices, load_history_indices};
use crate::{StageCheckpoint, StageId};
use reth_config::config::{EtlConfig, IndexHistoryConfig};
use reth_db_api::{
    models::{storage_sharded_key::StorageShardedKey, AddressStorageKey, BlockNumberAddress},
    table::Decode,
    tables,
    transaction::DbTxMut,
};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_provider::EitherWriter;
use reth_provider::{
    DBProvider, HistoryWriter, NodePrimitivesProvider, PruneCheckpointReader,
    PruneCheckpointWriter, RocksDBProviderFactory, StorageSettingsCache,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment};
use reth_stages_api::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use std::fmt::Debug;
use tracing::info;

/// Stage is indexing history the account changesets generated in
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
}

impl IndexStorageHistoryStage {
    /// Create new instance of [`IndexStorageHistoryStage`].
    pub const fn new(
        config: IndexHistoryConfig,
        etl_config: EtlConfig,
        prune_mode: Option<PruneMode>,
    ) -> Self {
        Self { commit_threshold: config.commit_threshold, prune_mode, etl_config }
    }
}

impl Default for IndexStorageHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000, prune_mode: None, etl_config: EtlConfig::default() }
    }
}

impl<Provider> Stage<Provider> for IndexStorageHistoryStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointWriter
        + HistoryWriter
        + PruneCheckpointReader
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
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

        // On first sync we might have history coming from genesis. We clear the table since it's
        // faster to rebuild from scratch.
        if first_sync {
            provider.tx_ref().clear::<tables::StoragesHistory>()?;
            range = 0..=*input.next_block_range().end();
        }

        info!(target: "sync::stages::index_storage_history::exec", ?first_sync, "Collecting indices");
        let collector =
            collect_history_indices::<_, tables::StorageChangeSets, tables::StoragesHistory, _>(
                provider,
                BlockNumberAddress::range(range.clone()),
                |AddressStorageKey((address, storage_key)), highest_block_number| {
                    StorageShardedKey::new(address, storage_key, highest_block_number)
                },
                |(key, value)| (key.block_number(), AddressStorageKey((key.address(), value.key))),
                &self.etl_config,
            )?;

        // Check if RocksDB is enabled for storage history
        #[cfg(all(unix, feature = "rocksdb"))]
        let use_rocksdb = provider.cached_storage_settings().storages_history_in_rocksdb;
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let use_rocksdb = false;

        info!(target: "sync::stages::index_storage_history::exec", ?use_rocksdb, "Loading indices into database");

        if use_rocksdb {
            #[cfg(all(unix, feature = "rocksdb"))]
            {
                // Create RocksDB batch
                let rocksdb = provider.rocksdb_provider();
                let mut rocksdb_batch = rocksdb.batch();

                if first_sync {
                    super::clear_rocksdb_table::<tables::StoragesHistory>(
                        &rocksdb,
                        &mut rocksdb_batch,
                    )?;
                }

                // Create writer that routes to RocksDB
                let mut writer = EitherWriter::new_storages_history(provider, rocksdb_batch)?;

                // Load indices using the RocksDB path
                super::load_storage_history_indices_via_writer(
                    &mut writer,
                    collector,
                    first_sync,
                    provider,
                )?;

                // Extract and register RocksDB batch for commit at provider level
                if let Some(batch) = writer.into_raw_rocksdb_batch() {
                    provider.set_pending_rocksdb_batch(batch);
                }
            }
        } else {
            // Keep existing MDBX path unchanged
            load_history_indices::<_, tables::StoragesHistory, _>(
                provider,
                collector,
                first_sync,
                |AddressStorageKey((address, storage_key)), highest_block_number| {
                    StorageShardedKey::new(address, storage_key, highest_block_number)
                },
                StorageShardedKey::decode_owned,
                |key| AddressStorageKey((key.address, key.sharded_key.key)),
            )?;
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

        // Check if RocksDB is enabled for storage history
        #[cfg(all(unix, feature = "rocksdb"))]
        let use_rocksdb = provider.cached_storage_settings().storages_history_in_rocksdb;
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let use_rocksdb = false;

        if use_rocksdb {
            #[cfg(all(unix, feature = "rocksdb"))]
            {
                use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
                use std::collections::BTreeSet;

                // Stream changesets directly into a set of affected (address, storage_key) pairs
                let mut affected_keys = BTreeSet::new();
                for entry in provider
                    .tx_ref()
                    .cursor_read::<tables::StorageChangeSets>()?
                    .walk_range(BlockNumberAddress::range(range.clone()))?
                {
                    let (block_address, storage_entry) = entry?;
                    affected_keys.insert((block_address.address(), storage_entry.key));
                }

                // Unwind using RocksDB
                super::unwind_storage_history_via_rocksdb(provider, affected_keys, *range.start())?;
            }
        } else {
            provider.unwind_storage_history_indices_range(BlockNumberAddress::range(range))?;
        }

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
        let mut close_full_list = (1..=LAST_BLOCK_IN_FULL_SHARD - 1).collect::<Vec<_>>();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StoragesHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD + 1, Some(LAST_BLOCK_IN_FULL_SHARD - 1));

        // verify
        close_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(LAST_BLOCK_IN_FULL_SHARD), close_full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1])
            ])
        );

        // unwind
        unwind(&db, LAST_BLOCK_IN_FULL_SHARD, LAST_BLOCK_IN_FULL_SHARD - 1);

        // verify initial state
        close_full_list.pop();
        let table = cast(db.table::<tables::StoragesHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list)]));
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
        use reth_provider::RocksDBProviderFactory;
        use reth_storage_api::StorageSettings;

        #[tokio::test]
        async fn execute_writes_storage_history_to_rocksdb_when_enabled() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // setup
            partial_setup(&db);

            // run
            run(&db, MAX_BLOCK, None);

            // Verify MDBX table is empty (data should be in RocksDB)
            let mdbx_count = db.table::<tables::StoragesHistory>().unwrap().len();
            assert_eq!(
                mdbx_count, 0,
                "MDBX StoragesHistory should be empty when RocksDB is enabled"
            );

            // Verify RocksDB has the data
            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "Storage history should exist in RocksDB");

            // Verify the block numbers are correct
            let list = result.unwrap();
            let blocks: Vec<u64> = list.iter().collect();
            assert!(!blocks.is_empty(), "Block list should not be empty");
        }

        #[tokio::test]
        async fn first_sync_clears_stale_rocksdb_storage_history() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // Seed RocksDB with a stale entry for a different address/storage key
            let rocksdb = db.factory.rocksdb_provider();
            let stale_address = address!("0x0000000000000000000000000000000000000002");
            let stale_storage_key =
                b256!("0x0000000000000000000000000000000000000000000000000000000000000002");
            let stale_key = StorageShardedKey {
                address: stale_address,
                sharded_key: ShardedKey { key: stale_storage_key, highest_block_number: u64::MAX },
            };
            rocksdb.put::<tables::StoragesHistory>(stale_key.clone(), &list(&[999])).unwrap();

            // setup
            partial_setup(&db);

            // run (first sync)
            run(&db, MAX_BLOCK, None);

            // Verify stale entry is removed
            let rocksdb = db.factory.rocksdb_provider();
            let stale = rocksdb.get::<tables::StoragesHistory>(stale_key).unwrap();
            assert!(
                stale.is_none(),
                "Stale RocksDB storage history should be cleared on first sync"
            );
        }

        #[tokio::test]
        async fn execute_writes_to_mdbx_when_rocksdb_disabled() {
            let db = TestStageDB::default();

            // Ensure RocksDB is disabled for storage history (default)
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(false),
            );

            // setup
            partial_setup(&db);

            // run
            run(&db, MAX_BLOCK, None);

            // Verify MDBX table has data
            let mdbx_count = db.table::<tables::StoragesHistory>().unwrap().len();
            assert!(
                mdbx_count > 0,
                "MDBX StoragesHistory should have data when RocksDB is disabled"
            );
        }

        /// Test incremental sync with RocksDB: run stage twice and verify indices are merged.
        #[tokio::test]
        async fn incremental_sync_merges_indices_in_rocksdb() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // First run: setup changesets for blocks 0-5 and index them
            let first_run_end: u64 = 5;
            db.commit(|tx| {
                for block in 0..=first_run_end {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::StorageChangeSets>(
                        block_number_address(block),
                        storage(STORAGE_KEY),
                    )?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage for first batch (first_sync = true since checkpoint is 0)
            run(&db, first_run_end, None);

            // Verify first run wrote to RocksDB
            let rocksdb = db.factory.rocksdb_provider();
            let first_result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(first_result.is_some(), "First run should write to RocksDB");
            let first_blocks: Vec<u64> = first_result.unwrap().iter().collect();
            assert_eq!(
                first_blocks.len(),
                (first_run_end + 1) as usize,
                "First run should have blocks 0-5"
            );

            // Second run: add changesets for blocks 6-10
            let second_run_end: u64 = 10;
            db.commit(|tx| {
                for block in (first_run_end + 1)..=second_run_end {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::StorageChangeSets>(
                        block_number_address(block),
                        storage(STORAGE_KEY),
                    )?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage for second batch (first_sync = false since checkpoint > 0)
            run(&db, second_run_end, Some(first_run_end));

            // Verify second run merged indices in RocksDB
            let rocksdb = db.factory.rocksdb_provider();
            let merged_result = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(merged_result.is_some(), "Second run should have data in RocksDB");
            let merged_blocks: Vec<u64> = merged_result.unwrap().iter().collect();

            // Should contain all blocks from 0 to 10
            let expected_blocks: Vec<u64> = (0..=second_run_end).collect();
            assert_eq!(
                merged_blocks, expected_blocks,
                "Incremental sync should merge all indices: expected {:?}, got {:?}",
                expected_blocks, merged_blocks
            );

            // Verify MDBX table is still empty
            let mdbx_count = db.table::<tables::StoragesHistory>().unwrap().len();
            assert_eq!(mdbx_count, 0, "MDBX should remain empty when RocksDB is enabled");
        }

        /// Test unwind removes storage history from RocksDB when enabled.
        #[tokio::test]
        async fn unwind_removes_storage_history_from_rocksdb() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // setup changesets for blocks 0-10
            db.commit(|tx| {
                for block in 0..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::StorageChangeSets>(
                        block_number_address(block),
                        storage(STORAGE_KEY),
                    )?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage to index blocks 0-10
            run(&db, 10, None);

            // Verify RocksDB has all blocks
            let rocksdb = db.factory.rocksdb_provider();
            let before_unwind = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(before_unwind.is_some(), "RocksDB should have data before unwind");
            let before_blocks: Vec<u64> = before_unwind.unwrap().iter().collect();
            assert_eq!(before_blocks, (0..=10).collect::<Vec<_>>(), "Should have blocks 0-10");

            // Unwind to block 5
            unwind(&db, 10, 5);

            // Verify RocksDB only contains blocks 0-5 (blocks <= unwind_to are kept)
            let rocksdb = db.factory.rocksdb_provider();
            let after_unwind = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(after_unwind.is_some(), "RocksDB should still have data after partial unwind");
            let after_blocks: Vec<u64> = after_unwind.unwrap().iter().collect();
            assert_eq!(
                after_blocks,
                (0..=5).collect::<Vec<_>>(),
                "After unwind to 5, should have blocks 0-5"
            );

            // Verify MDBX table is still empty
            let mdbx_count = db.table::<tables::StoragesHistory>().unwrap().len();
            assert_eq!(mdbx_count, 0, "MDBX should remain empty during RocksDB unwind");
        }

        /// Test unwind to genesis removes all storage history from RocksDB.
        #[tokio::test]
        async fn unwind_to_genesis_clears_rocksdb_storage_history() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // setup changesets for blocks 0-5
            db.commit(|tx| {
                for block in 0..=5u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::StorageChangeSets>(
                        block_number_address(block),
                        storage(STORAGE_KEY),
                    )?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage
            run(&db, 5, None);

            // Verify RocksDB has data
            let rocksdb = db.factory.rocksdb_provider();
            let before = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(before.is_some(), "RocksDB should have data before unwind");

            // Unwind to genesis (block 0)
            unwind(&db, 5, 0);

            // Verify RocksDB still has block 0 (blocks <= unwind_to are kept)
            let rocksdb = db.factory.rocksdb_provider();
            let after = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(after.is_some(), "RocksDB should still have block 0 after unwind to genesis");
            let after_blocks: Vec<u64> = after.unwrap().iter().collect();
            assert_eq!(after_blocks, vec![0], "After unwind to 0, should only have block 0");
        }

        /// Test unwind with no changesets in range is a no-op.
        #[tokio::test]
        async fn unwind_with_no_changesets_is_noop() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // Setup changesets only for blocks 0-5 (leave gap at 6-10)
            db.commit(|tx| {
                for block in 0..=5u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::StorageChangeSets>(
                        block_number_address(block),
                        storage(STORAGE_KEY),
                    )?;
                }
                // Add block body indices for 6-10 but NO changesets
                for block in 6..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage to index blocks 0-10
            run(&db, 10, None);

            // Record state before unwind
            let rocksdb = db.factory.rocksdb_provider();
            let before = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            let before_blocks: Vec<u64> = before.unwrap().iter().collect();

            // Unwind from 10 to 7 (range 8-10 has no changesets)
            unwind(&db, 10, 7);

            // Verify RocksDB data is unchanged (no changesets in unwind range)
            let rocksdb = db.factory.rocksdb_provider();
            let after = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            let after_blocks: Vec<u64> = after.unwrap().iter().collect();
            assert_eq!(
                before_blocks, after_blocks,
                "Data should be unchanged when no changesets in unwind range"
            );
        }

        /// Test unwind preserves unrelated storage keys.
        #[tokio::test]
        async fn unwind_preserves_unrelated_storage_keys() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            let key_1 = B256::with_last_byte(1);
            let key_2 = B256::with_last_byte(2);

            // Setup: key_1 has changes in blocks 0-10, key_2 only in blocks 0-5
            db.commit(|tx| {
                for block in 0..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    // key_1 changes in all blocks
                    tx.put::<tables::StorageChangeSets>(
                        block_number_address(block),
                        storage(key_1),
                    )?;
                    // key_2 only changes in blocks 0-5
                    if block <= 5 {
                        tx.put::<tables::StorageChangeSets>(
                            block_number_address(block),
                            storage(key_2),
                        )?;
                    }
                }
                Ok(())
            })
            .unwrap();

            // Run stage
            run(&db, 10, None);

            // Record key_2 state before unwind
            let rocksdb = db.factory.rocksdb_provider();
            let shard_key_2 = StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: key_2, highest_block_number: u64::MAX },
            };
            let before_2 = rocksdb.get::<tables::StoragesHistory>(shard_key_2.clone()).unwrap();
            let before_2_blocks: Vec<u64> = before_2.unwrap().iter().collect();

            // Unwind from 10 to 7 (only key_1 has changes in 8-10)
            unwind(&db, 10, 7);

            // Verify key_2 is unchanged (no changes in unwind range)
            let rocksdb = db.factory.rocksdb_provider();
            let after_2 = rocksdb.get::<tables::StoragesHistory>(shard_key_2).unwrap();
            let after_2_blocks: Vec<u64> = after_2.unwrap().iter().collect();
            assert_eq!(
                before_2_blocks, after_2_blocks,
                "Key 2 should be unchanged when it has no changes in unwind range"
            );

            // Verify key_1 was properly unwound
            let shard_key_1 = StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: key_1, highest_block_number: u64::MAX },
            };
            let after_1 = rocksdb.get::<tables::StoragesHistory>(shard_key_1).unwrap();
            let after_1_blocks: Vec<u64> = after_1.unwrap().iter().collect();
            assert_eq!(
                after_1_blocks,
                (0..=7).collect::<Vec<_>>(),
                "Key 1 should have blocks 0-7 after unwind"
            );
        }

        /// Test unwind deletes entry when all blocks are removed.
        #[tokio::test]
        async fn unwind_deletes_entry_when_all_blocks_removed() {
            let db = TestStageDB::default();

            // Enable RocksDB for storage history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_storages_history_in_rocksdb(true),
            );

            // Setup: only add changesets for blocks 5-10
            db.commit(|tx| {
                for block in 0..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    if block >= 5 {
                        tx.put::<tables::StorageChangeSets>(
                            block_number_address(block),
                            storage(STORAGE_KEY),
                        )?;
                    }
                }
                Ok(())
            })
            .unwrap();

            // Run stage
            run(&db, 10, None);

            // Verify RocksDB has data
            let rocksdb = db.factory.rocksdb_provider();
            let before = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(before.is_some(), "RocksDB should have data before unwind");

            // Unwind to block 4 (removes blocks 5-10, which is ALL the data)
            unwind(&db, 10, 4);

            // Verify entry is deleted (no blocks left)
            let rocksdb = db.factory.rocksdb_provider();
            let after = rocksdb.get::<tables::StoragesHistory>(shard(u64::MAX)).unwrap();
            assert!(after.is_none(), "Entry should be deleted when all blocks are removed");
        }
    }
}
