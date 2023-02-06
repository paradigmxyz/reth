use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::{Database, DatabaseGAT},
    models::{sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey},
    tables,
    transaction::{DbTx, DbTxMut, DbTxMutGAT},
    TransitionList,
};
use reth_primitives::{Address, TransitionId, H256};
use std::{collections::BTreeMap, fmt::Debug};
use tracing::*;

const INDEX_STORAGE_HISTORY: StageId = StageId("IndexStorageHistory");

/// Stage is indexing history the account changesets generated in
/// [`ExecutionStage`][crate::stages::ExecutionStage]. For more information
/// on index sharding take a look at [`tables::StorageHistory`].
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

        let storage_chageset = tx
            .cursor_read::<tables::StorageChangeSet>()?
            .walk(Some((from_transition, Address::zero()).into()))?
            .take_while(|res| {
                res.as_ref().map(|(k, _)| k.transition_id() < to_transition).unwrap_or_default()
            })
            .collect::<Result<Vec<_>, _>>()?;

        // fold all storages to one set of changes
        let storage_changeset_lists = storage_chageset.into_iter().fold(
            BTreeMap::new(),
            |mut storages: BTreeMap<(Address, H256), Vec<u64>>, (index, storage)| {
                storages
                    .entry((index.address(), storage.key))
                    .or_default()
                    .push(index.transition_id());
                storages
            },
        );

        for ((address, storage_key), mut indices) in storage_changeset_lists {
            let mut last_shard = take_last_storage_shard(tx, address, storage_key)?;
            last_shard.append(&mut indices);

            // chunk indices and insert them in shards of N size.
            let mut chunks = last_shard
                .iter()
                .chunks(NUM_OF_INDICES_IN_SHARD)
                .into_iter()
                .map(|chunks| chunks.map(|i| *i as usize).collect::<Vec<usize>>())
                .collect::<Vec<_>>();
            let last_chunk = chunks.pop();

            // chunk indices and insert them in shards of N size.
            chunks.into_iter().try_for_each(|list| {
                tx.put::<tables::StorageHistory>(
                    StorageShardedKey::new(
                        address,
                        storage_key,
                        *list.last().expect("Chuck does not return empty list") as TransitionId,
                    ),
                    TransitionList::new(list).expect("Indices are presorted and not empty"),
                )
            })?;
            // Insert last list with u64::MAX
            if let Some(last_list) = last_chunk {
                tx.put::<tables::StorageHistory>(
                    StorageShardedKey::new(address, storage_key, u64::MAX),
                    TransitionList::new(last_list).expect("Indices are presorted and not empty"),
                )?;
            }
        }

        info!(target: "sync::stages::index_storage_history", "Stage finished");
        Ok(ExecOutput { stage_progress: to_block, done: true })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        info!(target: "sync::stages::index_account_history", to_block = input.unwind_to, "Unwinding");
        let from_transition_rev = tx.get_block_transition(input.unwind_to)?;
        let to_transition_rev = tx.get_block_transition(input.stage_progress)?;

        let mut cursor = tx.cursor_write::<tables::StorageHistory>()?;

        let storage_changesets = tx
            .cursor_read::<tables::StorageChangeSet>()?
            .walk(Some((from_transition_rev, Address::zero()).into()))?
            .take_while(|res| {
                res.as_ref().map(|(k, _)| k.transition_id() < to_transition_rev).unwrap_or_default()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let last_indices = storage_changesets
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
            );
        for ((address, storage_key), rem_index) in last_indices {
            let shard_part =
                unwind_storage_history_shards::<DB>(&mut cursor, address, storage_key, rem_index)?;

            // check last shard_part, if present, items needs to be reinserted.
            if !shard_part.is_empty() {
                // there are items in list
                tx.put::<tables::StorageHistory>(
                    StorageShardedKey::new(address, storage_key, u64::MAX),
                    TransitionList::new(shard_part)
                        .expect("There is at least one element in list and it is sorted."),
                )?;
            }
        }
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

/// Load last shard and check if it is full and remove if it is not. If list is empty, last shard
/// was full or there is no shards at all.
pub fn take_last_storage_shard<DB: Database>(
    tx: &Transaction<'_, DB>,
    address: Address,
    storage_key: H256,
) -> Result<Vec<u64>, StageError> {
    let mut cursor = tx.cursor_read::<tables::StorageHistory>()?;
    let last = cursor.seek_exact(StorageShardedKey::new(address, storage_key, u64::MAX))?;
    if let Some((storage_shard_key, list)) = last {
        // delete old shard so new one can be inserted.
        tx.delete::<tables::StorageHistory>(storage_shard_key, None)?;
        let list = list.iter(0).map(|i| i as u64).collect::<Vec<_>>();
        return Ok(list)
    }
    Ok(Vec::new())
}

/// Unwind all history shards. For boundary shard, remove it from database and
/// return last part of shard with still valid items. If all full shard were removed, return list
/// would be empty but this does not mean that there is none shard left but that there is no
/// splitted shards.
pub fn unwind_storage_history_shards<DB: Database>(
    cursor: &mut <<DB as DatabaseGAT<'_>>::TXMut as DbTxMutGAT<'_>>::CursorMut<
        tables::StorageHistory,
    >,
    address: Address,
    storage_key: H256,
    transition_id: TransitionId,
) -> Result<Vec<usize>, StageError> {
    let mut item = cursor.seek_exact(StorageShardedKey::new(address, storage_key, u64::MAX))?;

    while let Some((storage_sharded_key, list)) = item {
        // there is no more shard for address
        if storage_sharded_key.address != address ||
            storage_sharded_key.sharded_key.key != storage_key
        {
            // there is no more shard for address and storage_key.
            break
        }
        cursor.delete_current()?;
        // check first item and if it is more and eq than `transition_id` delete current
        // item.
        let first = list.iter(0).next().expect("List can't empty");
        if first >= transition_id as usize {
            item = cursor.prev()?;
            continue
        } else if transition_id <= storage_sharded_key.sharded_key.highest_transition_id {
            // if first element is in scope whole list would be removed.
            // so at least this first element is present.
            return Ok(list.iter(0).take_while(|i| *i < transition_id as usize).collect::<Vec<_>>())
        } else {
            return Ok(list.iter(0).collect::<Vec<_>>())
        }
    }
    Ok(Vec::new())
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
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![4, 6]),]));

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
        assert_eq!(table, BTreeMap::from([(shard(u64::MAX), vec![1, 2, 3, 4, 6]),]));

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
        let mut input = ExecInput::default();
        input.previous_stage = Some((PREV_STAGE_ID, 5));

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
            BTreeMap::from([(shard(3), full_list.clone()), (shard(u64::MAX), vec![4, 6])])
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
        close_full_list.push(6);
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
            BTreeMap::from([(shard(4), close_full_list.clone()), (shard(u64::MAX), vec![6])])
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
                (shard(u64::MAX), vec![2, 3, 4, 6])
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
}
