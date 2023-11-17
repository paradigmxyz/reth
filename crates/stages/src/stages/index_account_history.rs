use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    PruneCheckpoint, PruneMode, PruneSegment,
};
use reth_provider::{
    AccountExtReader, DatabaseProviderRW, HistoryWriter, PruneCheckpointReader,
    PruneCheckpointWriter,
};
use std::fmt::Debug;

/// Stage is indexing history the account changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`reth_db::tables::AccountHistory`]
#[derive(Debug)]
pub struct IndexAccountHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
    /// Pruning configuration.
    pub prune_mode: Option<PruneMode>,
}

impl IndexAccountHistoryStage {
    /// Create new instance of [IndexAccountHistoryStage].
    pub fn new(commit_threshold: u64, prune_mode: Option<PruneMode>) -> Self {
        Self { commit_threshold, prune_mode }
    }
}

impl Default for IndexAccountHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000, prune_mode: None }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for IndexAccountHistoryStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexAccountHistory
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        mut input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        if let Some((target_prunable_block, prune_mode)) = self
            .prune_mode
            .map(|mode| mode.prune_target_block(input.target(), PruneSegment::AccountHistory))
            .transpose()?
            .flatten()
        {
            if target_prunable_block > input.checkpoint().block_number {
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
        }

        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }

        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);

        let indices = provider.changed_accounts_and_blocks_with_range(range.clone())?;
        // Insert changeset to history index
        provider.insert_account_history_index(indices)?;

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(*range.end()), done: is_final_range })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        provider.unwind_account_history_indices(range)?;

        // from HistoryIndex higher than that number.
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(unwind_progress) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner,
    };
    use itertools::Itertools;
    use reth_db::{
        cursor::DbCursorRO,
        models::{
            sharded_key, sharded_key::NUM_OF_INDICES_IN_SHARD, AccountBeforeTx, ShardedKey,
            StoredBlockBodyIndices,
        },
        tables,
        transaction::{DbTx, DbTxMut},
        BlockNumberList,
    };
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_changeset_range, random_contract_account_range},
    };
    use reth_primitives::{address, Address, BlockNumber, PruneMode, B256, MAINNET};
    use reth_provider::ProviderFactory;
    use std::collections::BTreeMap;

    const ADDRESS: Address = address!("0000000000000000000000000000000000000001");

    const LAST_BLOCK_IN_FULL_SHARD: BlockNumber = NUM_OF_INDICES_IN_SHARD as BlockNumber;
    const MAX_BLOCK: BlockNumber = NUM_OF_INDICES_IN_SHARD as BlockNumber + 2;

    fn acc() -> AccountBeforeTx {
        AccountBeforeTx { address: ADDRESS, info: None }
    }

    /// Shard for account
    fn shard(shard_index: u64) -> ShardedKey<Address> {
        ShardedKey { key: ADDRESS, highest_block_number: shard_index }
    }

    fn list(list: &[u64]) -> BlockNumberList {
        BlockNumberList::new(list).unwrap()
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

    fn partial_setup(tx: &TestTransaction) {
        // setup
        tx.commit(|tx| {
            for block in 0..=MAX_BLOCK {
                tx.put::<tables::BlockBodyIndices>(
                    block,
                    StoredBlockBodyIndices { tx_count: 3, ..Default::default() },
                )?;
                // setup changeset that is going to be applied to history index
                tx.put::<tables::AccountChangeSet>(block, acc())?;
            }
            Ok(())
        })
        .unwrap()
    }

    async fn run(tx: &TestTransaction, run_to: u64, input_checkpoint: Option<BlockNumber>) {
        let input = ExecInput {
            target: Some(run_to),
            checkpoint: input_checkpoint
                .map(|block_number| StageCheckpoint { block_number, stage_checkpoint: None }),
        };
        let mut stage = IndexAccountHistoryStage::default();
        let factory = ProviderFactory::new(tx.tx.as_ref(), MAINNET.clone());
        let provider = factory.provider_rw().unwrap();
        let out = stage.execute(&provider, input).await.unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(run_to), done: true });
        provider.commit().unwrap();
    }

    async fn unwind(tx: &TestTransaction, unwind_from: u64, unwind_to: u64) {
        let input = UnwindInput {
            checkpoint: StageCheckpoint::new(unwind_from),
            unwind_to,
            ..Default::default()
        };
        let mut stage = IndexAccountHistoryStage::default();
        let factory = ProviderFactory::new(tx.tx.as_ref(), MAINNET.clone());
        let provider = factory.provider_rw().unwrap();
        let out = stage.unwind(&provider, input).await.unwrap();
        assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) });
        provider.commit().unwrap();
    }

    #[tokio::test]
    async fn insert_index_to_empty() {
        // init
        let tx = TestTransaction::default();

        // setup
        partial_setup(&tx);

        // run
        run(&tx, 3, None).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3])]));

        // unwind
        unwind(&tx, 3, 0).await;

        // verify initial state
        let table = tx.table::<tables::AccountHistory>().unwrap();
        assert!(table.is_empty());
    }

    #[tokio::test]
    async fn insert_index_to_not_empty_shard() {
        // init
        let tx = TestTransaction::default();

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5, Some(3)).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 5])]));

        // unwind
        unwind(&tx, 5, 3).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3])]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let tx = TestTransaction::default();
        let full_list = (1..=LAST_BLOCK_IN_FULL_SHARD).collect::<Vec<_>>();
        assert_eq!(full_list.len(), NUM_OF_INDICES_IN_SHARD);

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, LAST_BLOCK_IN_FULL_SHARD + 2, Some(LAST_BLOCK_IN_FULL_SHARD)).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(LAST_BLOCK_IN_FULL_SHARD), full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1, LAST_BLOCK_IN_FULL_SHARD + 2])
            ])
        );

        // unwind
        unwind(&tx, LAST_BLOCK_IN_FULL_SHARD + 2, LAST_BLOCK_IN_FULL_SHARD).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), full_list)]));
    }

    #[tokio::test]
    async fn insert_index_to_fill_shard() {
        // init
        let tx = TestTransaction::default();
        let mut almost_full_list = (1..=LAST_BLOCK_IN_FULL_SHARD - 2).collect::<Vec<_>>();

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&almost_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, LAST_BLOCK_IN_FULL_SHARD, Some(LAST_BLOCK_IN_FULL_SHARD - 2)).await;

        // verify
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD - 1);
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list.clone())]));

        // unwind
        unwind(&tx, LAST_BLOCK_IN_FULL_SHARD, LAST_BLOCK_IN_FULL_SHARD - 2).await;

        // verify initial state
        almost_full_list.pop();
        almost_full_list.pop();
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list)]));

        // verify initial state
    }

    #[tokio::test]
    async fn insert_index_second_half_shard() {
        // init
        let tx = TestTransaction::default();
        let mut almost_full_list = (1..=LAST_BLOCK_IN_FULL_SHARD - 1).collect::<Vec<_>>();

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&almost_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, LAST_BLOCK_IN_FULL_SHARD + 1, Some(LAST_BLOCK_IN_FULL_SHARD - 1)).await;

        // verify
        almost_full_list.push(LAST_BLOCK_IN_FULL_SHARD);
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(LAST_BLOCK_IN_FULL_SHARD), almost_full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1])
            ])
        );

        // unwind
        unwind(&tx, LAST_BLOCK_IN_FULL_SHARD, LAST_BLOCK_IN_FULL_SHARD - 1).await;

        // verify initial state
        almost_full_list.pop();
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), almost_full_list)]));
    }

    #[tokio::test]
    async fn insert_index_to_third_shard() {
        // init
        let tx = TestTransaction::default();
        let full_list = (1..=LAST_BLOCK_IN_FULL_SHARD).collect::<Vec<_>>();

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::AccountHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::AccountHistory>(
                shard(u64::MAX),
                list(&[LAST_BLOCK_IN_FULL_SHARD + 1]),
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        run(&tx, LAST_BLOCK_IN_FULL_SHARD + 2, Some(LAST_BLOCK_IN_FULL_SHARD + 1)).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1, LAST_BLOCK_IN_FULL_SHARD + 2])
            ])
        );

        // unwind
        unwind(&tx, LAST_BLOCK_IN_FULL_SHARD + 2, LAST_BLOCK_IN_FULL_SHARD + 1).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![LAST_BLOCK_IN_FULL_SHARD + 1])
            ])
        );
    }

    #[tokio::test]
    async fn insert_index_with_prune_mode() {
        // init
        let tx = TestTransaction::default();

        // setup
        tx.commit(|tx| {
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
            tx.put::<tables::AccountChangeSet>(20, acc()).unwrap();
            tx.put::<tables::AccountChangeSet>(36, acc()).unwrap();
            tx.put::<tables::AccountChangeSet>(100, acc()).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        let input = ExecInput { target: Some(20000), ..Default::default() };
        let mut stage = IndexAccountHistoryStage {
            prune_mode: Some(PruneMode::Before(36)),
            ..Default::default()
        };
        let factory = ProviderFactory::new(tx.tx.as_ref(), MAINNET.clone());
        let provider = factory.provider_rw().unwrap();
        let out = stage.execute(&provider, input).await.unwrap();
        assert_eq!(out, ExecOutput { checkpoint: StageCheckpoint::new(20000), done: true });
        provider.commit().unwrap();

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![36, 100])]));

        // unwind
        unwind(&tx, 20000, 0).await;

        // verify initial state
        let table = tx.table::<tables::AccountHistory>().unwrap();
        assert!(table.is_empty());
    }

    stage_test_suite_ext!(IndexAccountHistoryTestRunner, index_account_history);

    struct IndexAccountHistoryTestRunner {
        pub(crate) tx: TestTransaction,
        commit_threshold: u64,
        prune_mode: Option<PruneMode>,
    }

    impl Default for IndexAccountHistoryTestRunner {
        fn default() -> Self {
            Self { tx: TestTransaction::default(), commit_threshold: 1000, prune_mode: None }
        }
    }

    impl StageTestRunner for IndexAccountHistoryTestRunner {
        type S = IndexAccountHistoryStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            Self::S { commit_threshold: self.commit_threshold, prune_mode: self.prune_mode }
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

            let blocks = random_block_range(&mut rng, start..=end, B256::ZERO, 0..3);

            let (changesets, _) = random_changeset_range(
                &mut rng,
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );

            // add block changeset from block 1.
            self.tx.insert_changesets(changesets, Some(start))?;

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

                let provider = self.tx.inner();
                let mut changeset_cursor =
                    provider.tx_ref().cursor_read::<tables::AccountChangeSet>()?;

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

                    chunks.into_iter().for_each(|list| {
                        result.insert(
                            ShardedKey::new(
                                address,
                                *list.last().expect("Chuck does not return empty list")
                                    as BlockNumber,
                            ),
                            list,
                        );
                    });

                    if let Some(last_list) = last_chunk {
                        result.insert(ShardedKey::new(address, u64::MAX), last_list);
                    };
                }

                let table = cast(self.tx.table::<tables::AccountHistory>().unwrap());
                assert_eq!(table, result);
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for IndexAccountHistoryTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            let table = self.tx.table::<tables::AccountHistory>().unwrap();
            assert!(table.is_empty());
            Ok(())
        }
    }
}
