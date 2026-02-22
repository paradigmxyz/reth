use super::collect_account_history_indices;
use crate::stages::utils::{collect_history_indices, load_account_history};
use reth_config::config::{EtlConfig, IndexHistoryConfig};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_db_api::Tables;
use reth_db_api::{models::ShardedKey, tables, transaction::DbTxMut};
use reth_provider::{
    DBProvider, EitherWriter, HistoryWriter, PruneCheckpointReader, PruneCheckpointWriter,
    RocksDBProviderFactory, StorageSettingsCache,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use std::fmt::Debug;
use tracing::info;

/// Stage is indexing history the account changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`tables::AccountsHistory`]
#[derive(Debug)]
pub struct IndexAccountHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
    /// Pruning configuration.
    pub prune_mode: Option<PruneMode>,
    /// ETL configuration
    pub etl_config: EtlConfig,
}

impl IndexAccountHistoryStage {
    /// Create new instance of [`IndexAccountHistoryStage`].
    pub const fn new(
        config: IndexHistoryConfig,
        etl_config: EtlConfig,
        prune_mode: Option<PruneMode>,
    ) -> Self {
        Self { commit_threshold: config.commit_threshold, etl_config, prune_mode }
    }
}

impl Default for IndexAccountHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000, prune_mode: None, etl_config: EtlConfig::default() }
    }
}

impl<Provider> Stage<Provider> for IndexAccountHistoryStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + HistoryWriter
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + reth_storage_api::ChangeSetReader
        + reth_provider::StaticFileProviderFactory
        + StorageSettingsCache
        + RocksDBProviderFactory,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexAccountHistory
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
                    PruneSegment::AccountHistory,
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
            if provider.get_prune_checkpoint(PruneSegment::AccountHistory)?.is_none() {
                provider.save_prune_checkpoint(
                    PruneSegment::AccountHistory,
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
                provider.rocksdb_provider().clear::<tables::AccountsHistory>()?;
            } else {
                provider.tx_ref().clear::<tables::AccountsHistory>()?;
            }
            range = 0..=*input.next_block_range().end();
        }

        info!(target: "sync::stages::index_account_history::exec", ?first_sync, ?use_rocksdb, "Collecting indices");

        let collector = if provider.cached_storage_settings().storage_v2 {
            // Use the provider-based collection that can read from static files.
            collect_account_history_indices(provider, range.clone(), &self.etl_config)?
        } else {
            collect_history_indices::<_, tables::AccountChangeSets, tables::AccountsHistory, _>(
                provider,
                range.clone(),
                ShardedKey::new,
                |(index, value)| (index, value.address),
                &self.etl_config,
            )?
        };

        info!(target: "sync::stages::index_account_history::exec", "Loading indices into database");

        provider.with_rocksdb_batch_auto_commit(|rocksdb_batch| {
            let mut writer = EitherWriter::new_accounts_history(provider, rocksdb_batch)?;
            load_account_history(collector, first_sync, &mut writer)
                .map_err(|e| reth_provider::ProviderError::other(Box::new(e)))?;
            Ok(((), writer.into_raw_rocksdb_batch()))
        })?;

        #[cfg(all(unix, feature = "rocksdb"))]
        if use_rocksdb {
            provider.commit_pending_rocksdb_batches()?;
            provider.rocksdb_provider().flush(&[Tables::AccountsHistory.name()])?;
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

        provider.unwind_account_history_indices_range(range)?;

        // from HistoryIndex higher than that number.
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
    use alloy_primitives::{address, Address, BlockNumber, B256};
    use itertools::Itertools;
    use reth_db_api::{
        cursor::DbCursorRO,
        models::{
            sharded_key, sharded_key::NUM_OF_INDICES_IN_SHARD, AccountBeforeTx,
            StoredBlockBodyIndices,
        },
        transaction::DbTx,
        BlockNumberList,
    };
    use reth_provider::{providers::StaticFileWriter, DatabaseProviderFactory};
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_contract_account_range,
        BlockRangeParams,
    };
    use std::collections::BTreeMap;

    const ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");

    const LAST_BLOCK_IN_FULL_SHARD: BlockNumber = NUM_OF_INDICES_IN_SHARD as BlockNumber;
    const MAX_BLOCK: BlockNumber = NUM_OF_INDICES_IN_SHARD as BlockNumber + 2;

    const fn acc() -> AccountBeforeTx {
        AccountBeforeTx { address: ADDRESS, info: None }
    }

    /// Shard for account
    const fn shard(shard_index: u64) -> ShardedKey<Address> {
        ShardedKey { key: ADDRESS, highest_block_number: shard_index }
    }

    fn list(list: &[u64]) -> BlockNumberList {
        BlockNumberList::new(list.iter().copied()).unwrap()
    }

    fn cast(
        table: Vec<(ShardedKey<Address>, BlockNumberList)>,
    ) -> BTreeMap<ShardedKey<Address>, Vec<u64>> {
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
                tx.put::<tables::AccountChangeSets>(block, acc())?;
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
        let mut stage = IndexAccountHistoryStage::default();
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
        let mut stage = IndexAccountHistoryStage::default();
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
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![0, 1, 2, 3])]));

        // unwind
        unwind(&db, 3, 0);

        // verify initial state
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![0])]));
    }

    #[tokio::test]
    async fn insert_index_to_not_empty_shard() {
        // init
        let db = TestStageDB::default();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::AccountsHistory>(shard(u64::MAX), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, 5, Some(3));

        // verify
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 5])]));

        // unwind
        unwind(&db, 5, 3);

        // verify initial state
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3])]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let db = TestStageDB::default();
        let full_list = (1..=LAST_BLOCK_IN_FULL_SHARD).collect::<Vec<_>>();
        assert_eq!(full_list.len(), NUM_OF_INDICES_IN_SHARD);

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::AccountsHistory>(shard(u64::MAX), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD + 2, Some(LAST_BLOCK_IN_FULL_SHARD));

        // verify
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
            tx.put::<tables::AccountsHistory>(shard(u64::MAX), list(&almost_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD, Some(LAST_BLOCK_IN_FULL_SHARD - 2));

        // verify
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD - 1);
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list.clone())]));

        // unwind
        unwind(&db, LAST_BLOCK_IN_FULL_SHARD, LAST_BLOCK_IN_FULL_SHARD - 2);

        // verify initial state
        almost_full_list.pop();
        almost_full_list.pop();
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
            tx.put::<tables::AccountsHistory>(shard(u64::MAX), list(&almost_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, LAST_BLOCK_IN_FULL_SHARD + 1, Some(LAST_BLOCK_IN_FULL_SHARD - 1));

        // verify
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
            tx.put::<tables::AccountsHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::AccountsHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::AccountsHistory>(
                shard(u64::MAX),
                list(&[LAST_BLOCK_IN_FULL_SHARD + 1]),
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        run(&db, LAST_BLOCK_IN_FULL_SHARD + 2, Some(LAST_BLOCK_IN_FULL_SHARD + 1));

        // verify
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
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
            tx.put::<tables::AccountChangeSets>(20, acc()).unwrap();
            tx.put::<tables::AccountChangeSets>(36, acc()).unwrap();
            tx.put::<tables::AccountChangeSets>(100, acc()).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        let input = ExecInput { target: Some(20000), ..Default::default() };
        let mut stage = IndexAccountHistoryStage {
            prune_mode: Some(PruneMode::Before(36)),
            ..Default::default()
        };
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(20000), done: true });
        provider.commit().unwrap();

        // verify
        let table = cast(db.table::<tables::AccountsHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![36, 100])]));

        // unwind
        unwind(&db, 20000, 0);

        // verify initial state
        let table = db.table::<tables::AccountsHistory>().unwrap();
        assert!(table.is_empty());
    }

    stage_test_suite_ext!(IndexAccountHistoryTestRunner, index_account_history);

    struct IndexAccountHistoryTestRunner {
        pub(crate) db: TestStageDB,
        commit_threshold: u64,
        prune_mode: Option<PruneMode>,
    }

    impl Default for IndexAccountHistoryTestRunner {
        fn default() -> Self {
            Self { db: TestStageDB::default(), commit_threshold: 1000, prune_mode: None }
        }
    }

    impl StageTestRunner for IndexAccountHistoryTestRunner {
        type S = IndexAccountHistoryStage;

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

    impl ExecuteStageTestRunner for IndexAccountHistoryTestRunner {
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
                0..256,
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
                    provider.tx_ref().cursor_read::<tables::AccountChangeSets>()?;

                let account_transitions =
                    changeset_cursor.walk_range(start_block..=end_block)?.try_fold(
                        BTreeMap::new(),
                        |mut accounts: BTreeMap<Address, Vec<u64>>,
                         entry|
                         -> Result<_, TestRunnerError> {
                            let (index, account) = entry?;
                            accounts.entry(account.address).or_default().push(index);
                            Ok(accounts)
                        },
                    )?;

                let mut result = BTreeMap::new();
                for (address, indices) in account_transitions {
                    // chunk indices and insert them in shards of N size.
                    let mut chunks = indices
                        .iter()
                        .chunks(sharded_key::NUM_OF_INDICES_IN_SHARD)
                        .into_iter()
                        .map(|chunks| chunks.copied().collect::<Vec<_>>())
                        .collect::<Vec<Vec<_>>>();
                    let last_chunk = chunks.pop();

                    for list in chunks {
                        result.insert(
                            ShardedKey::new(
                                address,
                                *list.last().expect("Chuck does not return empty list")
                                    as BlockNumber,
                            ),
                            list,
                        );
                    }

                    if let Some(last_list) = last_chunk {
                        result.insert(ShardedKey::new(address, u64::MAX), last_list);
                    };
                }

                let table = cast(self.db.table::<tables::AccountsHistory>().unwrap());
                assert_eq!(table, result);
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for IndexAccountHistoryTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            let table = self.db.table::<tables::AccountsHistory>().unwrap();
            assert!(table.is_empty());
            Ok(())
        }
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    mod rocksdb_tests {
        use super::*;
        use reth_provider::{
            providers::StaticFileWriter, RocksDBProviderFactory, StaticFileProviderFactory,
        };
        use reth_static_file_types::StaticFileSegment;
        use reth_storage_api::StorageSettings;

        /// Sets up v2 account test data: writes block body indices to MDBX and
        /// account changesets to static files (matching realistic v2 layout).
        fn setup_v2_account_data(db: &TestStageDB, block_range: std::ops::RangeInclusive<u64>) {
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
                static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();
            for block in block_range {
                writer.append_account_changeset(vec![acc()], block).unwrap();
            }
            writer.commit().unwrap();
        }

        /// Test that when `account_history_in_rocksdb` is enabled, the stage
        /// writes account history indices to `RocksDB` instead of MDBX.
        #[tokio::test]
        async fn execute_writes_to_rocksdb_when_enabled() {
            let db = TestStageDB::default();
            setup_v2_account_data(&db, 0..=10);

            let input = ExecInput { target: Some(10), ..Default::default() };
            let mut stage = IndexAccountHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
            provider.commit().unwrap();

            // Verify MDBX table is empty (data should be in RocksDB)
            let mdbx_table = db.table::<tables::AccountsHistory>().unwrap();
            assert!(
                mdbx_table.is_empty(),
                "MDBX AccountsHistory should be empty when RocksDB is enabled"
            );

            // Verify RocksDB has the data
            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should contain account history");

            let block_list = result.unwrap();
            let blocks: Vec<u64> = block_list.iter().collect();
            assert_eq!(blocks, (0..=10).collect::<Vec<_>>());
        }

        /// Test that unwind works correctly when `account_history_in_rocksdb` is enabled.
        #[tokio::test]
        async fn unwind_works_when_rocksdb_enabled() {
            let db = TestStageDB::default();
            setup_v2_account_data(&db, 0..=10);

            let input = ExecInput { target: Some(10), ..Default::default() };
            let mut stage = IndexAccountHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
            provider.commit().unwrap();

            // Verify RocksDB has blocks 0-10 before unwind
            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should have data before unwind");
            let blocks_before: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks_before, (0..=10).collect::<Vec<_>>());

            // Unwind to block 5 (remove blocks 6-10)
            let unwind_input =
                UnwindInput { checkpoint: StageCheckpoint::new(10), unwind_to: 5, bad_block: None };
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.unwind(&provider, unwind_input).unwrap();
            assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(5) });
            provider.commit().unwrap();

            // Verify RocksDB now only has blocks 0-5 (blocks 6-10 removed)
            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should still have data after unwind");
            let blocks_after: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks_after, (0..=5).collect::<Vec<_>>(), "Should only have blocks 0-5");
        }

        /// Test incremental sync merges new data with existing shards.
        #[tokio::test]
        async fn execute_incremental_sync() {
            let db = TestStageDB::default();
            setup_v2_account_data(&db, 0..=10);

            let input = ExecInput { target: Some(5), ..Default::default() };
            let mut stage = IndexAccountHistoryStage::default();
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(5), done: true });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some());
            let blocks: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks, (0..=5).collect::<Vec<_>>());

            let input = ExecInput { target: Some(10), checkpoint: Some(StageCheckpoint::new(5)) };
            let provider = db.factory.database_provider_rw().unwrap();
            let out = stage.execute(&provider, input).unwrap();
            assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
            provider.commit().unwrap();

            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "RocksDB should have merged data");
            let blocks: Vec<u64> = result.unwrap().iter().collect();
            assert_eq!(blocks, (0..=10).collect::<Vec<_>>());
        }
    }
}
