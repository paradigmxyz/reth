use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::{
    cursor::DbCursorRO, database::Database, models::BlockNumberAddress, tables, transaction::DbTx,
    DatabaseError,
};
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

/// Stage is indexing history the account changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`reth_db::tables::StorageHistory`].
#[derive(Debug)]
pub struct IndexStorageHistoryStage {
    /// Number of blocks after which the control
    /// flow will be returned to the pipeline for commit.
    pub commit_threshold: u64,
}

impl Default for IndexStorageHistoryStage {
    fn default() -> Self {
        Self { commit_threshold: 100_000 }
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for IndexStorageHistoryStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::IndexStorageHistory
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let target = input.target();
        let (range, is_final_range) = input.next_block_range_with_threshold(self.commit_threshold);

        if range.is_empty() {
            return Ok(ExecOutput::done(target))
        }

        let mut stage_checkpoint = stage_checkpoint(tx, input.checkpoint(), &range)?;

        let indices = tx.get_storage_transition_ids_from_changeset(range.clone())?;
        let changesets = indices.values().map(|blocks| blocks.len() as u64).sum::<u64>();

        tx.insert_storage_history_index(indices)?;

        stage_checkpoint.progress.processed += changesets;

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
        let (range, unwind_progress, _) =
            input.unwind_block_range_with_threshold(self.commit_threshold);

        let changesets = tx.unwind_storage_history_indices(BlockNumberAddress::range(range))?;

        let checkpoint =
            if let Some(mut stage_checkpoint) = input.checkpoint.index_history_stage_checkpoint() {
                stage_checkpoint.progress.processed -= changesets as u64;
                StageCheckpoint::new(unwind_progress)
                    .with_index_history_stage_checkpoint(stage_checkpoint)
            } else {
                StageCheckpoint::new(unwind_progress)
            };

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
/// [tables::StorageChangeSet] table within the given block range.
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
                    total: tx.deref().entries::<tables::StorageChangeSet>()? as u64,
                },
            }
        }
        _ => IndexHistoryCheckpoint {
            block_range: CheckpointBlockRange::from(range),
            progress: EntitiesCheckpoint {
                processed: tx
                    .cursor_read::<tables::StorageChangeSet>()?
                    .walk_range(BlockNumberAddress::range(0..=checkpoint.block_number))?
                    .count() as u64,
                total: tx.deref().entries::<tables::StorageChangeSet>()? as u64,
            },
        },
    })
}

#[cfg(test)]
mod tests {

    use assert_matches::assert_matches;
    use std::collections::BTreeMap;

    use super::*;
    use crate::test_utils::TestTransaction;
    use reth_db::{
        models::{
            storage_sharded_key::{StorageShardedKey, NUM_OF_INDICES_IN_SHARD},
            BlockNumberAddress, ShardedKey, StoredBlockBodyIndices,
        },
        tables,
        transaction::DbTxMut,
        BlockNumberList,
    };
    use reth_primitives::{hex_literal::hex, StorageEntry, H160, H256, U256};

    const ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000001"));
    const STORAGE_KEY: H256 =
        H256(hex!("0000000000000000000000000000000000000000000000000000000000000001"));

    fn storage(key: H256) -> StorageEntry {
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
            tx.put::<tables::StorageChangeSet>(trns(4), storage(STORAGE_KEY)).unwrap();
            tx.put::<tables::StorageChangeSet>(trns(5), storage(STORAGE_KEY)).unwrap();
            Ok(())
        })
        .unwrap()
    }

    async fn run(tx: &TestTransaction, run_to: u64) {
        let input = ExecInput { target: Some(run_to), ..Default::default() };
        let mut stage = IndexStorageHistoryStage::default();
        let mut tx = tx.inner();
        let out = stage.execute(&mut tx, input).await.unwrap();
        assert_eq!(
            out,
            ExecOutput {
                checkpoint: StageCheckpoint::new(5).with_index_history_stage_checkpoint(
                    IndexHistoryCheckpoint {
                        block_range: CheckpointBlockRange { from: input.next_block(), to: run_to },
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
        let mut stage = IndexStorageHistoryStage::default();
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
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![4, 5]),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = tx.table::<tables::StorageHistory>().unwrap();
        assert!(table.is_empty());
    }

    #[tokio::test]
    async fn insert_index_to_not_empty_shard() {
        // init
        let tx = TestTransaction::default();

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 5]),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3]),]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let tx = TestTransaction::default();
        let _input = ExecInput { target: Some(5), ..Default::default() };

        // change does not matter only that account is present in changeset.
        let full_list = vec![3; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([(shard(3), full_list.clone()), (shard(u64::MAX), vec![4, 5])])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
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
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        close_full_list.push(4);
        close_full_list.push(5);
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), close_full_list.clone()),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        close_full_list.pop();
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
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
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        close_full_list.push(4);
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(
            table,
            BTreeMap::from([(shard(4), close_full_list.clone()), (shard(u64::MAX), vec![5])])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
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
            tx.put::<tables::StorageHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::StorageHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::StorageHistory>(shard(u64::MAX), list(&[2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
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
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
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
            tx.put::<tables::StorageChangeSet>(
                BlockNumberAddress((1, H160(hex!("0000000000000000000000000000000000000001")))),
                storage(H256(hex!(
                    "0000000000000000000000000000000000000000000000000000000000000001"
                ))),
            )
            .unwrap();
            tx.put::<tables::StorageChangeSet>(
                BlockNumberAddress((1, H160(hex!("0000000000000000000000000000000000000001")))),
                storage(H256(hex!(
                    "0000000000000000000000000000000000000000000000000000000000000002"
                ))),
            )
            .unwrap();
            tx.put::<tables::StorageChangeSet>(
                BlockNumberAddress((1, H160(hex!("0000000000000000000000000000000000000002")))),
                storage(H256(hex!(
                    "0000000000000000000000000000000000000000000000000000000000000001"
                ))),
            )
            .unwrap();
            tx.put::<tables::StorageChangeSet>(
                BlockNumberAddress((2, H160(hex!("0000000000000000000000000000000000000001")))),
                storage(H256(hex!(
                    "0000000000000000000000000000000000000000000000000000000000000001"
                ))),
            )
            .unwrap();
            tx.put::<tables::StorageChangeSet>(
                BlockNumberAddress((2, H160(hex!("0000000000000000000000000000000000000001")))),
                storage(H256(hex!(
                    "0000000000000000000000000000000000000000000000000000000000000002"
                ))),
            )
            .unwrap();
            tx.put::<tables::StorageChangeSet>(
                BlockNumberAddress((2, H160(hex!("0000000000000000000000000000000000000002")))),
                storage(H256(hex!(
                    "0000000000000000000000000000000000000000000000000000000000000001"
                ))),
            )
            .unwrap();
            Ok(())
        })
        .unwrap();

        assert_matches!(
            stage_checkpoint(&tx.inner(), StageCheckpoint::new(1), &(1..=2)).unwrap(),
            IndexHistoryCheckpoint {
                block_range: CheckpointBlockRange { from: 1, to: 2 },
                progress: EntitiesCheckpoint { processed: 3, total: 6 }
            }
        );
    }
}
