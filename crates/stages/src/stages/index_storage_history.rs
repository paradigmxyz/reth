use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::{sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey},
    tables,
    transaction::{DbTx, DbTxMut},
    TransitionList,
};
use reth_primitives::{Address, TransitionId, H256};
use std::{collections::BTreeMap, fmt::Debug};
use tracing::*;

const INDEX_STORAGE_HISTORY: StageId = StageId("IndexStorageHistoryStage");

/// Account hashing stage hashes plain account.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
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
        INDEX_STORAGE_HISTORY
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        // read storge changeset, merge it into one changeset and calculate account hashes.
        let from_transition = tx.get_block_transition(stage_progress)?;

        // NOTE: can probably done more probabilistic take of bundles with transition but it is
        // guess game for later. Transitions better reflect amount of work.
        let to_block =
            std::cmp::min(stage_progress + self.commit_threshold, previous_stage_progress);
        let to_transition = tx.get_block_transition(to_block)?;

        tx.cursor_read::<tables::StorageChangeSet>()?
            .walk((from_transition, Address::zero()).into())?
            .take_while(|res| {
                res.as_ref().map(|(k, _)| k.transition_id() < to_transition).unwrap_or_default()
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // fold all storages to one set of changes
            .fold(
                BTreeMap::new(),
                |mut storages: BTreeMap<(Address, H256), Vec<u64>>, (index, storage)| {
                    storages
                        .entry((index.address(), storage.key))
                        .or_default()
                        .push(index.transition_id());
                    storages
                },
            )
            .into_iter()
            // insert indexes to StorageHistory.
            .try_for_each(|((address, storage_key), mut indices)| -> Result<(), StageError> {
                // load last shard and check if it is full, remove last shard and append indices.
                let indices = if let Some((storage_shard_key, list)) =
                    tx.get_storage_history_biggest_sharded_index(address, storage_key)?
                {
                    if list.len() >= NUM_OF_INDICES_IN_SHARD {
                        // if latest shard is full, just append new indices
                        indices
                    } else {
                        // delete old shard so new one can be inserted.
                        tx.delete::<tables::StorageHistory>(storage_shard_key, None)?;
                        let mut list = list.iter(0).map(|i| i as u64).collect::<Vec<_>>();
                        list.append(&mut indices);
                        list
                    }
                } else {
                    // if presently there isn't any shard insert all indices
                    indices
                };
                // chunk indices and insert them in shards of N size.
                indices.into_iter().chunks(NUM_OF_INDICES_IN_SHARD).into_iter().try_for_each(
                    |chuck| {
                        let list = chuck.map(|i| i as usize).collect::<Vec<_>>();
                        let biggest_id =
                            *list.last().expect("Chuck does not return empty list") as TransitionId;
                        let list =
                            TransitionList::new(list).expect("Indices are presorted and not empty");
                        tx.put::<tables::StorageHistory>(
                            StorageShardedKey::new(address, storage_key, biggest_id),
                            list,
                        )
                    },
                )?;
                // get
                Ok(())
            })?;

        info!(target: "sync::stages::index_storage_history", "Stage finished");
        Ok(ExecOutput { stage_progress: to_block, done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let from_transition_rev = tx.get_block_transition(input.unwind_to)?;
        let to_transition_rev = tx.get_block_transition(input.stage_progress)?;

        tx.cursor_read::<tables::StorageChangeSet>()?
            .walk((from_transition_rev, Address::zero()).into())?
            .take_while(|res| {
                res.as_ref().map(|(k, _)| k.transition_id() < to_transition_rev).unwrap_or_default()
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // reverse so we can get lowest transition id where we need to unwind account.
            .rev()
            // fold all storages and get last transition index
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<(Address, H256), u64>, (index, storage)| {
                    // we just need address and lowest transition id.
                    accounts.insert((index.address(), storage.key), index.transition_id());
                    accounts
                },
            )
            .into_iter()
            // try to unwind the index
            .try_for_each(|((address, storage_key), rem_index)| -> Result<(), StageError> {
                let mut cursor = tx.cursor_write::<tables::StorageHistory>()?;
                let _last_shard =
                    cursor.seek_exact(StorageShardedKey::new(address, storage_key, u64::MAX))?;

                let mut boundary = None;
                while let Some((storage_sharded_key, list)) = cursor.prev()? {
                    if storage_sharded_key.address != address ||
                        storage_sharded_key.sharded_key.key != storage_key
                    {
                        // there is no more shard for address and storage_key.
                        break
                    }
                    // check first item and if it is more and eq than `rem_index` delete current
                    // item.
                    let first = list.iter(0).next().expect("List can't empty");
                    if first >= rem_index as usize {
                        cursor.delete_current()?;
                    } else if rem_index <= storage_sharded_key.sharded_key.highest_transition_id {
                        // if eq, last element needs to be removed.
                        cursor.delete_current()?;
                        boundary = Some(list);
                        break
                    } else {
                        break
                    }
                }

                // check boundary, if present some items in current list needs to be removed.
                if let Some(old_list) = boundary {
                    let new_list = old_list
                        .iter(0)
                        .take_while(|i| *i < rem_index as usize)
                        .collect::<Vec<_>>();
                    // While loop above checks if first and last element [first, .., last]
                    // if first element is in scope whole list would be removed.
                    // so at least this first element is present.
                    let biggest_index =
                        *new_list.last().expect("There is at least one element in list");
                    let new_list = TransitionList::new(new_list)
                        .expect("There is at least one element in list and it is sorted.");
                    tx.put::<tables::StorageHistory>(
                        StorageShardedKey::new(address, storage_key, biggest_index as u64),
                        new_list,
                    )?;
                }
                Ok(())
            })?;
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}
#[cfg(test)]
mod tests {

    use super::*;
    use crate::test_utils::{TestTransaction, PREV_STAGE_ID};
    use reth_db::models::{ShardedKey, TransitionIdAddress};
    use reth_primitives::{hex_literal::hex, StorageEntry, H160, U256};

    const ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000001"));
    const STORAGE_KEY: H256 =
        H256(hex!("0000000000000000000000000000000000000000000000000000000000000001"));

    fn storage(key: H256) -> StorageEntry {
        // Value is not used in indexing stage.
        StorageEntry { key, value: U256::ZERO }
    }

    fn trns(transition_id: u64) -> TransitionIdAddress {
        TransitionIdAddress((transition_id, ADDRESS))
    }

    /// Shard for account
    fn shard(shard_index: u64) -> StorageShardedKey {
        StorageShardedKey {
            address: ADDRESS,
            sharded_key: ShardedKey { key: STORAGE_KEY, highest_transition_id: shard_index },
        }
    }

    fn list(list: &[usize]) -> TransitionList {
        TransitionList::new(list).unwrap()
    }

    fn cast(
        table: Vec<(StorageShardedKey, TransitionList)>,
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
            tx.put::<tables::BlockTransitionIndex>(0, 3).unwrap();
            tx.put::<tables::BlockTransitionIndex>(5, 7).unwrap();

            // setup changeset that are going to be applied to history index
            tx.put::<tables::StorageChangeSet>(trns(4), storage(STORAGE_KEY)).unwrap();
            tx.put::<tables::StorageChangeSet>(trns(6), storage(STORAGE_KEY)).unwrap();
            Ok(())
        })
        .unwrap()
    }

    async fn run(tx: &TestTransaction, run_to: u64) {
        let mut input = ExecInput::default();
        input.previous_stage = Some((PREV_STAGE_ID, run_to));
        let mut stage = IndexStorageHistoryStage::default();
        let mut tx = tx.inner();
        let out = stage.execute(&mut tx, input).await.unwrap();
        assert_eq!(out, ExecOutput { stage_progress: 5, done: true });
        tx.commit().unwrap();
    }

    async fn unwind(tx: &TestTransaction, unwind_from: u64, unwind_to: u64) {
        let mut input = UnwindInput::default();
        input.stage_progress = unwind_from;
        input.unwind_to = unwind_to;
        let mut stage = IndexStorageHistoryStage::default();
        let mut tx = tx.inner();
        let out = stage.unwind(&mut tx, input).await.unwrap();
        assert_eq!(out, UnwindOutput { stage_progress: unwind_to });
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
        assert_eq!(table, BTreeMap::from([(shard(6), vec![4, 6]),]));

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
            tx.put::<tables::StorageHistory>(shard(3), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(6), vec![1, 2, 3, 4, 6]),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(3), vec![1, 2, 3]),]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let tx = TestTransaction::default();
        let mut input = ExecInput::default();
        input.previous_stage = Some((PREV_STAGE_ID, 5));

        // change does not matter only that account is present in changeset.
        let full_list = vec![3; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(3), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(3), full_list.clone()), (shard(6), vec![4, 6])]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(3), full_list),]));
    }

    #[tokio::test]
    async fn insert_index_to_fill_shard() {
        // init
        let tx = TestTransaction::default();
        let mut close_full_list = vec![1; NUM_OF_INDICES_IN_SHARD - 2];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::StorageHistory>(shard(3), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        close_full_list.push(4);
        close_full_list.push(6);
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(6), close_full_list.clone()),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        close_full_list.pop();
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(1), close_full_list),]));

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
            tx.put::<tables::StorageHistory>(shard(1), list(&close_full_list)).unwrap();
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
            BTreeMap::from([(shard(4), close_full_list.clone()), (shard(6), vec![6])])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        let table = cast(tx.table::<tables::StorageHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(1), close_full_list),]));
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
            tx.put::<tables::StorageHistory>(shard(3), list(&[2, 3])).unwrap();
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
                (shard(6), vec![2, 3, 4, 6])
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
                (shard(3), vec![2, 3])
            ])
        );
    }
}
