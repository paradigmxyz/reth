use crate::stages::utils::collect_history_indices;

use super::{collect_account_history_indices, load_accounts_history_indices};
use alloy_primitives::{Address, BlockNumber};
use reth_config::config::{EtlConfig, IndexHistoryConfig};
use reth_db_api::{
    cursor::DbCursorRO,
    models::ShardedKey,
    table::Decode,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_provider::{
    make_rocksdb_batch_arg, make_rocksdb_provider, register_rocksdb_batch, DBProvider,
    EitherWriter, HistoryWriter, PruneCheckpointReader, PruneCheckpointWriter,
    RocksDBProviderFactory, StorageSettingsCache,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_storage_api::NodePrimitivesProvider;
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
        + NodePrimitivesProvider
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

        // On first sync we might have history coming from genesis. We clear the table since it's
        // faster to rebuild from scratch.
        if first_sync {
            provider.tx_ref().clear::<tables::AccountsHistory>()?;
            range = 0..=*input.next_block_range().end();
        }

        info!(target: "sync::stages::index_account_history::exec", ?first_sync, "Collecting indices");

        let collector = if provider.cached_storage_settings().account_changesets_in_static_files {
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
        load_accounts_history_indices(
            provider,
            collector,
            first_sync,
            ShardedKey::new,
            ShardedKey::<Address>::decode_owned,
            |key| key.key,
        )?;

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

        // Create EitherWriter for account history
        #[allow(clippy::let_unit_value)]
        let rocksdb = make_rocksdb_provider(provider);
        #[allow(clippy::let_unit_value)]
        let rocksdb_batch = make_rocksdb_batch_arg(&rocksdb);
        let mut writer = EitherWriter::new_accounts_history(provider, rocksdb_batch)?;

        // Read changesets to identify what to unwind
        let changesets = provider
            .tx_ref()
            .cursor_read::<tables::AccountChangeSets>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;

        // Group by address and find minimum block for each
        // We only need to unwind once per address using the LOWEST block number
        // since unwind removes all indices >= that block
        let mut account_keys: std::collections::HashMap<Address, BlockNumber> =
            std::collections::HashMap::new();
        for (block_number, account) in changesets {
            account_keys
                .entry(account.address)
                .and_modify(|min_bn| *min_bn = (*min_bn).min(block_number))
                .or_insert(block_number);
        }

        // Unwind each account's history shards (once per unique address)
        for (address, min_block) in account_keys {
            super::utils::unwind_accounts_history_shards(&mut writer, address, min_block)?;
        }

        // Register RocksDB batch for commit
        register_rocksdb_batch(provider, writer);

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
}

#[cfg(all(test, unix, feature = "rocksdb"))]
mod rocksdb_stage_tests {
    use super::*;
    use crate::test_utils::TestStageDB;
    use reth_db_api::tables;
    use reth_provider::{DatabaseProviderFactory, RocksDBProviderFactory};
    use reth_storage_api::StorageSettings;

    /// Test that `IndexAccountHistoryStage` writes to `RocksDB` when enabled.
    #[test]
    fn test_index_account_history_writes_to_rocksdb() {
        let db = TestStageDB::default();
        db.factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Setup changesets (blocks 1-10, skip 0 to avoid genesis edge case)
        db.commit(|tx| {
            for block in 1..=10u64 {
                tx.put::<tables::BlockBodyIndices>(
                    block,
                    reth_db_api::models::StoredBlockBodyIndices {
                        tx_count: 3,
                        ..Default::default()
                    },
                )?;
                tx.put::<tables::AccountChangeSets>(
                    block,
                    reth_db_api::models::AccountBeforeTx {
                        address: alloy_primitives::address!(
                            "0x0000000000000000000000000000000000000001"
                        ),
                        info: None,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();

        // Execute stage from checkpoint 0 (will process blocks 1-10)
        let input = ExecInput { target: Some(10), checkpoint: Some(StageCheckpoint::new(0)) };
        let mut stage = IndexAccountHistoryStage::default();
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
        provider.commit().unwrap();

        // Verify data is in RocksDB
        let rocksdb = db.factory.rocksdb_provider();
        let count =
            rocksdb.iter::<tables::AccountsHistory>().unwrap().filter_map(|r| r.ok()).count();
        assert!(count > 0, "Expected data in RocksDB, found {count} entries");

        // Verify MDBX AccountsHistory is empty (data went to RocksDB)
        let mdbx_table = db.table::<tables::AccountsHistory>().unwrap();
        assert!(mdbx_table.is_empty(), "MDBX should be empty when RocksDB is enabled");
    }

    /// Test that `IndexAccountHistoryStage` unwind clears `RocksDB` data.
    #[test]
    fn test_index_account_history_unwind_clears_rocksdb() {
        let db = TestStageDB::default();
        db.factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Setup changesets (blocks 1-10, skip 0 to avoid genesis edge case)
        db.commit(|tx| {
            for block in 1..=10u64 {
                tx.put::<tables::BlockBodyIndices>(
                    block,
                    reth_db_api::models::StoredBlockBodyIndices {
                        tx_count: 3,
                        ..Default::default()
                    },
                )?;
                tx.put::<tables::AccountChangeSets>(
                    block,
                    reth_db_api::models::AccountBeforeTx {
                        address: alloy_primitives::address!(
                            "0x0000000000000000000000000000000000000001"
                        ),
                        info: None,
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();

        // Execute stage from checkpoint 0 (will process blocks 1-10)
        let input = ExecInput { target: Some(10), checkpoint: Some(StageCheckpoint::new(0)) };
        let mut stage = IndexAccountHistoryStage::default();
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(10), done: true });
        provider.commit().unwrap();

        // Verify data exists in RocksDB
        let rocksdb = db.factory.rocksdb_provider();
        let before_count =
            rocksdb.iter::<tables::AccountsHistory>().unwrap().filter_map(|r| r.ok()).count();
        assert!(before_count > 0, "Expected data in RocksDB before unwind");

        // Unwind to block 0 (removes blocks 1-10, leaving nothing)
        let unwind_input = UnwindInput {
            checkpoint: StageCheckpoint::new(10),
            unwind_to: 0,
            ..Default::default()
        };
        let provider = db.factory.database_provider_rw().unwrap();
        let out = stage.unwind(&provider, unwind_input).unwrap();
        assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(0) });
        provider.commit().unwrap();

        // Verify RocksDB is cleared (no block 0 data exists)
        let rocksdb = db.factory.rocksdb_provider();
        let after_count =
            rocksdb.iter::<tables::AccountsHistory>().unwrap().filter_map(|r| r.ok()).count();
        assert_eq!(after_count, 0, "RocksDB should be empty after unwind to 0");
    }
}
