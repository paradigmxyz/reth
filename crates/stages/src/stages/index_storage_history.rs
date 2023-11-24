use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::{database::Database, models::BlockNumberAddress};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    PruneCheckpoint, PruneMode, PruneSegment,
};
use reth_provider::{
    DatabaseProviderRW, HistoryWriter, PruneCheckpointReader, PruneCheckpointWriter, StorageReader,
};
use std::fmt::Debug;

/// Stage is indexing history the account changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`reth_db::tables::StorageHistory`].
#[derive(Debug)]
pub struct IndexStorageHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
    /// Pruning configuration.
    pub prune_mode: Option<PruneMode>,
}

impl IndexStorageHistoryStage {
    /// Create new instance of [IndexStorageHistoryStage].
    pub fn new(commit_threshold: u64, prune_mode: Option<PruneMode>) -> Self {
        Self { commit_threshold, prune_mode }
    }
}

impl Default for IndexStorageHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000, prune_mode: None }
    }
}

impl<DB: Database> Stage<DB> for IndexStorageHistoryStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexStorageHistory
    }

    /// Execute the stage.
    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        mut input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if let Some((target_prunable_block, prune_mode)) = self
            .prune_mode
            .map(|mode| mode.prune_target_block(input.target(), PruneSegment::StorageHistory))
            .transpose()?
            .flatten()
        {
            if target_prunable_block > input.checkpoint().block_number {
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
        }

        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);

        let indices = provider.changed_storages_and_blocks_with_range(range.clone())?;
        provider.insert_storage_history_index(indices)?;

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(*range.end()), done: is_final_range })
    }

    /// Unwind the stage.
    fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        provider.unwind_storage_history_indices(BlockNumberAddress::range(range))?;

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
    use itertools::Itertools;
    use reth_db::{
        cursor::DbCursorRO,
        models::{
            sharded_key,
            storage_sharded_key::{StorageShardedKey, NUM_OF_INDICES_IN_SHARD},
            BlockNumberAddress, ShardedKey, StoredBlockBodyIndices,
        },
        tables,
        transaction::{DbTx, DbTxMut},
        BlockNumberList,
    };
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_changeset_range, random_contract_account_range},
    };
    use reth_primitives::{
        address, b256, Address, BlockNumber, PruneMode, StorageEntry, B256, U256,
    };
    use std::collections::BTreeMap;

    const ADDRESS: Address = address!("0000000000000000000000000000000000000001");
    const STORAGE_KEY: B256 =
        b256!("0000000000000000000000000000000000000000000000000000000000000001");

    fn storage(key: B256) -> StorageEntry {
        // Value is not used in indexing stage.
        StorageEntry { key, value: U256::ZERO }
    }

    fn trns(transition_id: u64) -> BlockNumberAddress {
        BlockNumberAddress((transition_id, ADDRESS))
    }

    /// Shard for account
    fn shard(shard_index: u64) -> StorageShardedKey {
        StorageShardedKey {
            address: ADDRESS,
            sharded_key: ShardedKey { key: STORAGE_KEY, highest_block_number: shard_index },
        }
    }

    fn list(list: &[usize]) -> BlockNumberList {
        BlockNumberList::new(list).unwrap()
    }

    fn cast(
        table: Vec<(StorageShardedKey, BlockNumberList)>,
    ) -> BTreeMap<StorageShardedKey, Vec<usize>> {
        table
            .into_iter()
            .map(|(k, v)| {
                let v = v.iter(0).collect();
                (k, v)
            })
            .collect()
    }

    fn partial_setup(db: &TestStageDB) {
        // setup
        db.commit(|tx| {
            // we just need first and last
            tx.put::<tables::BlockBodyIndices>(
                0,
                StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
            )
            .unwrap();

            tx.put::<tables::BlockBodyIndices>(
                5,
                StoredBlockBodyIndices { tx_count: 5, ..Default::default() },
            )
            .unwrap();

            // setup changeset that are going to be applied to history index
            tx.put::<tables::StorageChangeSet>(trns(4), storage(STORAGE_KEY)).unwrap();
            tx.put::<tables::StorageChangeSet>(trns(5), storage(STORAGE_KEY)).unwrap();
            Ok(())
        })
        .unwrap()
    }

    fn run(db: &TestStageDB, run_to: u64) {
        let input = ExecInput { target: Some(run_to), ..Default::default() };
        let mut stage = IndexStorageHistoryStage::default();
        let provider = db.factory.provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(5), done: true });
        provider.commit().unwrap();
    }

    fn unwind(db: &TestStageDB, unwind_from: u64, unwind_to: u64) {
        let input = UnwindInput {
            checkpoint: StageCheckpoint::new(unwind_from),
            unwind_to,
            ..Default::default()
        };
        let mut stage = IndexStorageHistoryStage::default();
        let provider = db.factory.provider_rw().unwrap();
        let out = stage.unwind(&provider, input).unwrap();
        assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) });
        provider.commit().unwrap();
    }

    #[tokio::test]
    async fn insert_index_to_empty() {
        // init
        let db = TestStageDB::default();

        // setup
        partial_setup(&db);

        // run
        run(&db, 5);

        // verify
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![4, 5]),]));

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        let table = db.table::<tables::StorageHistory>().unwrap();
        assert!(table.is_empty());
    }

    #[tokio::test]
    async fn insert_index_to_not_empty_shard() {
        // init
        let db = TestStageDB::default();

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, 5);

        // verify
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 5]),]));

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3]),]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let db = TestStageDB::default();
        let _input = ExecInput { target: Some(5), ..Default::default() };

        // change does not matter only that account is present in changeset.
        let full_list = vec![3; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, 5);

        // verify
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([(shard(3), full_list.clone()), (shard(u64::MAX), vec![4, 5])])
        );

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), full_list)]));
    }

    #[tokio::test]
    async fn insert_index_to_fill_shard() {
        // init
        let db = TestStageDB::default();
        let mut close_full_list = vec![1; NUM_OF_INDICES_IN_SHARD - 2];

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, 5);

        // verify
        close_full_list.push(4);
        close_full_list.push(5);
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list.clone()),]));

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        close_full_list.pop();
        close_full_list.pop();
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list),]));

        // verify initial state
    }

    #[tokio::test]
    async fn insert_index_second_half_shard() {
        // init
        let db = TestStageDB::default();
        let mut close_full_list = vec![1; NUM_OF_INDICES_IN_SHARD - 1];

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&db, 5);

        // verify
        close_full_list.push(4);
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([(shard(4), close_full_list.clone()), (shard(u64::MAX), vec![5])])
        );

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        close_full_list.pop();
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list),]));
    }

    #[tokio::test]
    async fn insert_index_to_third_shard() {
        // init
        let db = TestStageDB::default();
        let full_list = vec![1; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&db);
        db.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::StorageHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&[2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        run(&db, 5);

        // verify
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![2, 3, 4, 5])
            ])
        );

        // unwind
        unwind(&db, 5, 0);

        // verify initial state
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![2, 3])
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
            tx.put::<tables::StorageChangeSet>(trns(20), storage(STORAGE_KEY)).unwrap();
            tx.put::<tables::StorageChangeSet>(trns(36), storage(STORAGE_KEY)).unwrap();
            tx.put::<tables::StorageChangeSet>(trns(100), storage(STORAGE_KEY)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        let input = ExecInput { target: Some(20000), ..Default::default() };
        let mut stage = IndexStorageHistoryStage {
            prune_mode: Some(PruneMode::Before(36)),
            ..Default::default()
        };
        let provider = db.factory.provider_rw().unwrap();
        let out = stage.execute(&provider, input).unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(20000), done: true });
        provider.commit().unwrap();

        // verify
        let table = cast(db.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![36, 100]),]));

        // unwind
        unwind(&db, 20000, 0);

        // verify initial state
        let table = db.table::<tables::StorageHistory>().unwrap();
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
            Self::S { commit_threshold: self.commit_threshold, prune_mode: self.prune_mode }
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

            let blocks = random_block_range(&mut rng, start..=end, B256::ZERO, 0..3);

            let (transitions, _) = random_changeset_range(
                &mut rng,
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );

            // add block changeset from block 1.
            self.db.insert_changesets(transitions, Some(start))?;

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
                    provider.tx_ref().cursor_read::<tables::StorageChangeSet>()?;

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
                        .map(|chunks| chunks.map(|i| *i as usize).collect::<Vec<usize>>())
                        .collect::<Vec<_>>();
                    let last_chunk = chunks.pop();

                    chunks.into_iter().for_each(|list| {
                        result.insert(
                            StorageShardedKey::new(
                                partial_key.0,
                                partial_key.1,
                                *list.last().expect("Chuck does not return empty list")
                                    as BlockNumber,
                            ),
                            list,
                        );
                    });

                    if let Some(last_list) = last_chunk {
                        result.insert(
                            StorageShardedKey::new(partial_key.0, partial_key.1, u64::MAX),
                            last_list,
                        );
                    };
                }

                let table = cast(self.db.table::<tables::StorageHistory>().unwrap());
                assert_eq!(table, result);
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for IndexStorageHistoryTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            let table = self.db.table::<tables::StorageHistory>().unwrap();
            assert!(table.is_empty());
            Ok(())
        }
    }
}
