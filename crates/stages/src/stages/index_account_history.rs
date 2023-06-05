use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx, DatabaseError};
use reth_primitives::{
    stage::{
        CheckpointBlockRange, EntitiesCheckpoint, IndexHistoryCheckpoint, StageCheckpoint, StageId,
    },
    BlockNumber,
};
use reth_provider::Transaction;
use std::{
    fmt::Debug,
    ops::{Deref, RangeInclusive},
};
use tracing::*;

/// Stage is indexing history the account changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`reth_db::tables::AccountHistory`]
#[derive(Debug)]
pub struct IndexAccountHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
}

impl Default for IndexAccountHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
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
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);

        if range.is_empty() {
            return Ok(ExecOutput::done(StageCheckpoint::new(*range.end())))
        }

        let mut stage_checkpoint = stage_checkpoint(tx, input.checkpoint(), &range)?;

        let indices = tx.get_account_transition_ids_from_changeset(range.clone())?;
        let changesets = indices.values().map(|blocks| blocks.len() as u64).sum::<u64>();

        // Insert changeset to history index
        tx.insert_account_history_index(indices)?;

        stage_checkpoint.progress.processed += changesets;

        info!(target: "sync::stages::index_account_history", stage_progress = *range.end(), is_final_range, "Stage iteration finished");
        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(*range.end())
                .with_index_history_stage_checkpoint(stage_checkpoint),
            done: is_final_range,
        })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let (range, unwind_progress, is_final_range) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        let changesets = tx.unwind_account_history_indices(range)?;

        let checkpoint =
            if let Some(mut stage_checkpoint) = input.checkpoint.index_history_stage_checkpoint() {
                stage_checkpoint.progress.processed -= changesets as u64;
                StageCheckpoint::new(unwind_progress)
                    .with_index_history_stage_checkpoint(stage_checkpoint)
            } else {
                StageCheckpoint::new(unwind_progress)
            };

        info!(target: "sync::stages::index_account_history", to_block = input.unwind_to, unwind_progress, is_final_range, "Unwind iteration finished");
        // from HistoryIndex higher than that number.
        Ok(UnwindOutput { checkpoint })
    }
}

/// The function proceeds as follows:
/// 1. It first checks if the checkpoint has an [IndexHistoryCheckpoint] that matches the given
/// block range. If it does, the function returns that checkpoint.
/// 2. If the checkpoint's block range end matches the current checkpoint's block number, it creates
/// a new [IndexHistoryCheckpoint] with the given block range and updates the progress with the
/// current progress.
/// 3. If none of the above conditions are met, it creates a new [IndexHistoryCheckpoint] with the
/// given block range and calculates the progress by counting the number of processed entries in the
/// [tables::AccountChangeSet] table within the given block range.
fn stage_checkpoint<DB: Database>(
    tx: &Transaction<'_, DB>,
    checkpoint: StageCheckpoint,
    range: &RangeInclusive<BlockNumber>,
) -> Result<IndexHistoryCheckpoint, DatabaseError> {
    Ok(match checkpoint.index_history_stage_checkpoint() {
        Some(stage_checkpoint @ IndexHistoryCheckpoint { block_range, .. })
            if block_range == CheckpointBlockRange::from(range) =>
        {
            stage_checkpoint
        }
        Some(IndexHistoryCheckpoint { block_range, progress })
            if block_range.to == checkpoint.block_number =>
        {
            IndexHistoryCheckpoint {
                block_range: CheckpointBlockRange::from(range),
                progress: EntitiesCheckpoint {
                    processed: progress.processed,
                    total: tx.deref().entries::<tables::AccountChangeSet>()? as u64,
                },
            }
        }
        _ => IndexHistoryCheckpoint {
            block_range: CheckpointBlockRange::from(range),
            progress: EntitiesCheckpoint {
                processed: tx
                    .cursor_read::<tables::AccountChangeSet>()?
                    .walk_range(0..=checkpoint.block_number)?
                    .count() as u64,
                total: tx.deref().entries::<tables::AccountChangeSet>()? as u64,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use std::collections::BTreeMap;

    use super::*;
    use crate::test_utils::{TestTransaction, PREV_STAGE_ID};
    use reth_db::{
        models::{
            sharded_key::NUM_OF_INDICES_IN_SHARD, AccountBeforeTx, ShardedKey,
            StoredBlockBodyIndices,
        },
        tables,
        transaction::DbTxMut,
        BlockNumberList,
    };
    use reth_primitives::{hex_literal::hex, H160};

    const ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000001"));

    fn acc() -> AccountBeforeTx {
        AccountBeforeTx { address: ADDRESS, info: None }
    }

    /// Shard for account
    fn shard(shard_index: u64) -> ShardedKey<H160> {
        ShardedKey { key: ADDRESS, highest_block_number: shard_index }
    }

    fn list(list: &[usize]) -> BlockNumberList {
        BlockNumberList::new(list).unwrap()
    }

    fn cast(
        table: Vec<(ShardedKey<H160>, BlockNumberList)>,
    ) -> BTreeMap<ShardedKey<H160>, Vec<usize>> {
        table
            .into_iter()
            .map(|(k, v)| {
                let v = v.iter(0).collect();
                (k, v)
            })
            .collect()
    }

    fn partial_setup(tx: &TestTransaction) {
        // setup
        tx.commit(|tx| {
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
            tx.put::<tables::AccountChangeSet>(4, acc()).unwrap();
            tx.put::<tables::AccountChangeSet>(5, acc()).unwrap();
            Ok(())
        })
        .unwrap()
    }

    async fn run(tx: &TestTransaction, run_to: u64) {
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(run_to))),
            ..Default::default()
        };
        let mut stage = IndexAccountHistoryStage::default();
        let mut tx = tx.inner();
        let out = stage.execute(&mut tx, input).await.unwrap();
        assert_eq!(
            out,
            ExecOutput {
                checkpoint: StageCheckpoint::new(5).with_index_history_stage_checkpoint(
                    IndexHistoryCheckpoint {
                        block_range: CheckpointBlockRange {
                            from: input.checkpoint().block_number + 1,
                            to: run_to
                        },
                        progress: EntitiesCheckpoint { processed: 2, total: 2 }
                    }
                ),
                done: true
            }
        );
        tx.commit().unwrap();
    }

    async fn unwind(tx: &TestTransaction, unwind_from: u64, unwind_to: u64) {
        let input = UnwindInput {
            checkpoint: StageCheckpoint::new(unwind_from),
            unwind_to,
            ..Default::default()
        };
        let mut stage = IndexAccountHistoryStage::default();
        let mut tx = tx.inner();
        let out = stage.unwind(&mut tx, input).await.unwrap();
        assert_eq!(out, UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) });
        tx.commit().unwrap();
    }

    #[tokio::test]
    async fn insert_index_to_empty() {
        // init
        let tx = TestTransaction::default();

        // setup
        partial_setup(&tx);

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![4, 5])]));

        // unwind
        unwind(&tx, 5, 0).await;

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
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 5]),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3]),]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let tx = TestTransaction::default();
        let full_list = vec![3; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([(shard(3), full_list.clone()), (shard(u64::MAX), vec![4, 5])])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), full_list)]));
    }

    #[tokio::test]
    async fn insert_index_to_fill_shard() {
        // init
        let tx = TestTransaction::default();
        let mut close_full_list = vec![1; NUM_OF_INDICES_IN_SHARD - 2];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        close_full_list.push(4);
        close_full_list.push(5);
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list.clone()),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        close_full_list.pop();
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list),]));

        // verify initial state
    }

    #[tokio::test]
    async fn insert_index_second_half_shard() {
        // init
        let tx = TestTransaction::default();
        let mut close_full_list = vec![1; NUM_OF_INDICES_IN_SHARD - 1];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        close_full_list.push(4);
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([(shard(4), close_full_list.clone()), (shard(u64::MAX), vec![5])])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list),]));
    }

    #[tokio::test]
    async fn insert_index_to_third_shard() {
        // init
        let tx = TestTransaction::default();
        let full_list = vec![1; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::AccountHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::AccountHistory>(shard(u64::MAX), list(&[2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![2, 3, 4, 5])
            ])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([
                (shard(1), full_list.clone()),
                (shard(2), full_list.clone()),
                (shard(u64::MAX), vec![2, 3])
            ])
        );
    }

    #[test]
    fn stage_checkpoint_recalculation() {
        let tx = TestTransaction::default();

        tx.commit(|tx| {
            tx.put::<tables::AccountChangeSet>(
                1,
                AccountBeforeTx {
                    address: H160(hex!("0000000000000000000000000000000000000001")),
                    info: None,
                },
            )
            .unwrap();
            tx.put::<tables::AccountChangeSet>(
                1,
                AccountBeforeTx {
                    address: H160(hex!("0000000000000000000000000000000000000002")),
                    info: None,
                },
            )
            .unwrap();
            tx.put::<tables::AccountChangeSet>(
                2,
                AccountBeforeTx {
                    address: H160(hex!("0000000000000000000000000000000000000001")),
                    info: None,
                },
            )
            .unwrap();
            tx.put::<tables::AccountChangeSet>(
                2,
                AccountBeforeTx {
                    address: H160(hex!("0000000000000000000000000000000000000002")),
                    info: None,
                },
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        assert_matches!(
            stage_checkpoint(&tx.inner(), StageCheckpoint::new(1), &(1..=2)).unwrap(),
            IndexHistoryCheckpoint {
                block_range: CheckpointBlockRange { from: 1, to: 2 },
                progress: EntitiesCheckpoint { processed: 2, total: 4 }
            }
        );
    }
}
