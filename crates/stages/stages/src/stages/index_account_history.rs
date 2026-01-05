use super::{collect_history_indices, load_history_indices};
use alloy_primitives::Address;
use reth_config::config::{EtlConfig, IndexHistoryConfig};
use reth_db_api::{models::ShardedKey, table::Decode, tables, transaction::DbTxMut};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_provider::EitherWriter;
use reth_provider::{
    DBProvider, HistoryWriter, NodePrimitivesProvider, PruneCheckpointReader,
    PruneCheckpointWriter, RocksDBProviderFactory, StorageSettingsCache,
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
        + StorageSettingsCache
        + RocksDBProviderFactory
        + NodePrimitivesProvider,
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

        // On first sync we might have history coming from genesis. We clear the table since it's
        // faster to rebuild from scratch.
        if first_sync {
            provider.tx_ref().clear::<tables::AccountsHistory>()?;
            range = 0..=*input.next_block_range().end();
        }

        info!(target: "sync::stages::index_account_history::exec", ?first_sync, "Collecting indices");
        let collector =
            collect_history_indices::<_, tables::AccountChangeSets, tables::AccountsHistory, _>(
                provider,
                range.clone(),
                ShardedKey::new,
                |(index, value)| (index, value.address),
                &self.etl_config,
            )?;

        // Check if RocksDB is enabled for account history
        #[cfg(all(unix, feature = "rocksdb"))]
        let use_rocksdb = provider.cached_storage_settings().account_history_in_rocksdb;
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let use_rocksdb = false;

        info!(target: "sync::stages::index_account_history::exec", ?use_rocksdb, "Loading indices into database");

        if use_rocksdb {
            #[cfg(all(unix, feature = "rocksdb"))]
            {
                // Create RocksDB batch
                let rocksdb = provider.rocksdb_provider();
                let mut rocksdb_batch = rocksdb.batch();

                if first_sync {
                    super::clear_rocksdb_table::<tables::AccountsHistory>(
                        &rocksdb,
                        &mut rocksdb_batch,
                    )?;
                }

                // Create writer that routes to RocksDB
                let mut writer = EitherWriter::new_accounts_history(provider, rocksdb_batch)?;

                // Load indices using the RocksDB path
                super::load_account_history_indices_via_writer(
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
            load_history_indices::<_, tables::AccountsHistory, _>(
                provider,
                collector,
                first_sync,
                ShardedKey::new,
                ShardedKey::<Address>::decode_owned,
                |key| key.key,
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

        // Check if RocksDB is enabled for account history
        #[cfg(all(unix, feature = "rocksdb"))]
        let use_rocksdb = provider.cached_storage_settings().account_history_in_rocksdb;
        #[cfg(not(all(unix, feature = "rocksdb")))]
        let use_rocksdb = false;

        if use_rocksdb {
            #[cfg(all(unix, feature = "rocksdb"))]
            {
                use reth_db_api::{cursor::DbCursorRO, transaction::DbTx};
                use std::collections::BTreeSet;

                // Stream changesets directly into a set of affected addresses
                let mut affected_addresses = BTreeSet::new();
                for entry in provider
                    .tx_ref()
                    .cursor_read::<tables::AccountChangeSets>()?
                    .walk_range(range.clone())?
                {
                    let (_block, account) = entry?;
                    affected_addresses.insert(account.address);
                }

                // Unwind using RocksDB
                super::unwind_account_history_via_rocksdb(
                    provider,
                    affected_addresses,
                    *range.start(),
                )?;
            }
        } else {
            provider.unwind_account_history_indices_range(range)?;
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
    use alloy_primitives::{address, BlockNumber, B256};
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
        use reth_provider::RocksDBProviderFactory;
        use reth_storage_api::StorageSettings;

        #[tokio::test]
        async fn execute_writes_account_history_to_rocksdb_when_enabled() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // setup
            partial_setup(&db);

            // run
            run(&db, MAX_BLOCK, None);

            // Verify MDBX table is empty (data should be in RocksDB)
            let mdbx_count = db.table::<tables::AccountsHistory>().unwrap().len();
            assert_eq!(
                mdbx_count, 0,
                "MDBX AccountsHistory should be empty when RocksDB is enabled"
            );

            // Verify RocksDB has the data
            let rocksdb = db.factory.rocksdb_provider();
            let result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(result.is_some(), "Account history should exist in RocksDB");

            // Verify the block numbers are correct
            let list = result.unwrap();
            let blocks: Vec<u64> = list.iter().collect();
            assert!(!blocks.is_empty(), "Block list should not be empty");
        }

        #[tokio::test]
        async fn first_sync_clears_stale_rocksdb_account_history() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // Seed RocksDB with a stale entry for a different address
            let rocksdb = db.factory.rocksdb_provider();
            let stale_address = address!("0x0000000000000000000000000000000000000002");
            let stale_key = ShardedKey { key: stale_address, highest_block_number: u64::MAX };
            rocksdb.put::<tables::AccountsHistory>(stale_key.clone(), &list(&[999])).unwrap();

            // setup
            partial_setup(&db);

            // run (first sync)
            run(&db, MAX_BLOCK, None);

            // Verify stale entry is removed
            let rocksdb = db.factory.rocksdb_provider();
            let stale = rocksdb.get::<tables::AccountsHistory>(stale_key).unwrap();
            assert!(
                stale.is_none(),
                "Stale RocksDB account history should be cleared on first sync"
            );
        }

        #[tokio::test]
        async fn execute_writes_to_mdbx_when_rocksdb_disabled() {
            let db = TestStageDB::default();

            // Ensure RocksDB is disabled for account history (default)
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(false),
            );

            // setup
            partial_setup(&db);

            // run
            run(&db, MAX_BLOCK, None);

            // Verify MDBX table has data
            let mdbx_count = db.table::<tables::AccountsHistory>().unwrap().len();
            assert!(
                mdbx_count > 0,
                "MDBX AccountsHistory should have data when RocksDB is disabled"
            );
        }

        /// Test incremental sync with RocksDB: run stage twice and verify indices are merged.
        #[tokio::test]
        async fn incremental_sync_merges_indices_in_rocksdb() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // First run: setup changesets for blocks 0-5 and index them
            let first_run_end: u64 = 5;
            db.commit(|tx| {
                for block in 0..=first_run_end {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::AccountChangeSets>(block, acc())?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage for first batch (first_sync = true since checkpoint is 0)
            run(&db, first_run_end, None);

            // Verify first run wrote to RocksDB
            let rocksdb = db.factory.rocksdb_provider();
            let first_result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
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
                    tx.put::<tables::AccountChangeSets>(block, acc())?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage for second batch (first_sync = false since checkpoint > 0)
            run(&db, second_run_end, Some(first_run_end));

            // Verify second run merged indices in RocksDB
            let rocksdb = db.factory.rocksdb_provider();
            let merged_result = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
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
            let mdbx_count = db.table::<tables::AccountsHistory>().unwrap().len();
            assert_eq!(mdbx_count, 0, "MDBX should remain empty when RocksDB is enabled");
        }

        /// Test unwind removes account history from RocksDB when enabled.
        #[tokio::test]
        async fn unwind_removes_account_history_from_rocksdb() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // setup changesets for blocks 0-10
            db.commit(|tx| {
                for block in 0..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::AccountChangeSets>(block, acc())?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage to index blocks 0-10
            run(&db, 10, None);

            // Verify RocksDB has all blocks
            let rocksdb = db.factory.rocksdb_provider();
            let before_unwind = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(before_unwind.is_some(), "RocksDB should have data before unwind");
            let before_blocks: Vec<u64> = before_unwind.unwrap().iter().collect();
            assert_eq!(before_blocks, (0..=10).collect::<Vec<_>>(), "Should have blocks 0-10");

            // Unwind to block 5
            unwind(&db, 10, 5);

            // Verify RocksDB only contains blocks 0-5 (blocks <= unwind_to are kept)
            let rocksdb = db.factory.rocksdb_provider();
            let after_unwind = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(after_unwind.is_some(), "RocksDB should still have data after partial unwind");
            let after_blocks: Vec<u64> = after_unwind.unwrap().iter().collect();
            assert_eq!(
                after_blocks,
                (0..=5).collect::<Vec<_>>(),
                "After unwind to 5, should have blocks 0-5"
            );

            // Verify MDBX table is still empty
            let mdbx_count = db.table::<tables::AccountsHistory>().unwrap().len();
            assert_eq!(mdbx_count, 0, "MDBX should remain empty during RocksDB unwind");
        }

        /// Test unwind to genesis removes all account history from RocksDB.
        #[tokio::test]
        async fn unwind_to_genesis_clears_rocksdb_account_history() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // setup changesets for blocks 0-5
            db.commit(|tx| {
                for block in 0..=5u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::AccountChangeSets>(block, acc())?;
                }
                Ok(())
            })
            .unwrap();

            // Run stage
            run(&db, 5, None);

            // Verify RocksDB has data
            let rocksdb = db.factory.rocksdb_provider();
            let before = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(before.is_some(), "RocksDB should have data before unwind");

            // Unwind to genesis (block 0)
            unwind(&db, 5, 0);

            // Verify RocksDB still has block 0 (blocks <= unwind_to are kept)
            let rocksdb = db.factory.rocksdb_provider();
            let after = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(after.is_some(), "RocksDB should still have block 0 after unwind to genesis");
            let after_blocks: Vec<u64> = after.unwrap().iter().collect();
            assert_eq!(after_blocks, vec![0], "After unwind to 0, should only have block 0");
        }

        /// Test unwind with no changesets in range is a no-op.
        #[tokio::test]
        async fn unwind_with_no_changesets_is_noop() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // Setup changesets only for blocks 0-5 (leave gap at 6-10)
            db.commit(|tx| {
                for block in 0..=5u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    tx.put::<tables::AccountChangeSets>(block, acc())?;
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
            let before = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            let before_blocks: Vec<u64> = before.unwrap().iter().collect();

            // Unwind from 10 to 7 (range 8-10 has no changesets)
            unwind(&db, 10, 7);

            // Verify RocksDB data is unchanged (no changesets in unwind range)
            let rocksdb = db.factory.rocksdb_provider();
            let after = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            let after_blocks: Vec<u64> = after.unwrap().iter().collect();
            assert_eq!(
                before_blocks, after_blocks,
                "Data should be unchanged when no changesets in unwind range"
            );
        }

        /// Test unwind preserves unrelated addresses.
        #[tokio::test]
        async fn unwind_preserves_unrelated_addresses() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            let address_a = address!("0x0000000000000000000000000000000000000001");
            let address_b = address!("0x0000000000000000000000000000000000000002");

            // Setup: address_a has changes in blocks 0-10, address_b only in blocks 0-5
            db.commit(|tx| {
                for block in 0..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    // address_a changes in all blocks
                    tx.put::<tables::AccountChangeSets>(
                        block,
                        AccountBeforeTx { address: address_a, info: None },
                    )?;
                    // address_b only changes in blocks 0-5
                    if block <= 5 {
                        tx.put::<tables::AccountChangeSets>(
                            block,
                            AccountBeforeTx { address: address_b, info: None },
                        )?;
                    }
                }
                Ok(())
            })
            .unwrap();

            // Run stage
            run(&db, 10, None);

            // Record address_b state before unwind
            let rocksdb = db.factory.rocksdb_provider();
            let key_b = ShardedKey { key: address_b, highest_block_number: u64::MAX };
            let before_b = rocksdb.get::<tables::AccountsHistory>(key_b.clone()).unwrap();
            let before_b_blocks: Vec<u64> = before_b.unwrap().iter().collect();

            // Unwind from 10 to 7 (only address_a has changes in 8-10)
            unwind(&db, 10, 7);

            // Verify address_b is unchanged (no changes in unwind range)
            let rocksdb = db.factory.rocksdb_provider();
            let after_b = rocksdb.get::<tables::AccountsHistory>(key_b).unwrap();
            let after_b_blocks: Vec<u64> = after_b.unwrap().iter().collect();
            assert_eq!(
                before_b_blocks, after_b_blocks,
                "Address B should be unchanged when it has no changes in unwind range"
            );

            // Verify address_a was properly unwound
            let key_a = ShardedKey { key: address_a, highest_block_number: u64::MAX };
            let after_a = rocksdb.get::<tables::AccountsHistory>(key_a).unwrap();
            let after_a_blocks: Vec<u64> = after_a.unwrap().iter().collect();
            assert_eq!(
                after_a_blocks,
                (0..=7).collect::<Vec<_>>(),
                "Address A should have blocks 0-7 after unwind"
            );
        }

        /// Test unwind deletes entry when all blocks are removed.
        #[tokio::test]
        async fn unwind_deletes_entry_when_all_blocks_removed() {
            let db = TestStageDB::default();

            // Enable RocksDB for account history
            db.factory.set_storage_settings_cache(
                StorageSettings::legacy().with_account_history_in_rocksdb(true),
            );

            // Setup: only add changesets for blocks 5-10
            db.commit(|tx| {
                for block in 0..=10u64 {
                    tx.put::<tables::BlockBodyIndices>(
                        block,
                        StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                    )?;
                    if block >= 5 {
                        tx.put::<tables::AccountChangeSets>(block, acc())?;
                    }
                }
                Ok(())
            })
            .unwrap();

            // Run stage
            run(&db, 10, None);

            // Verify RocksDB has data
            let rocksdb = db.factory.rocksdb_provider();
            let before = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(before.is_some(), "RocksDB should have data before unwind");

            // Unwind to block 4 (removes blocks 5-10, which is ALL the data)
            unwind(&db, 10, 4);

            // Verify entry is deleted (no blocks left)
            let rocksdb = db.factory.rocksdb_provider();
            let after = rocksdb.get::<tables::AccountsHistory>(shard(u64::MAX)).unwrap();
            assert!(after.is_none(), "Entry should be deleted when all blocks are removed");
        }
    }
}
