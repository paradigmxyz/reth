use crate::{
    db::Transaction, ExecInput, ExecOutput, Stage, StageError, StageId, UnwindInput, UnwindOutput,
};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::{Database, DatabaseGAT},
    models::{sharded_key::NUM_OF_INDICES_IN_SHARD, ShardedKey},
    tables,
    transaction::{DbTx, DbTxMut, DbTxMutGAT},
    TransitionList,
};
use reth_primitives::{Address, TransitionId};
use std::{collections::BTreeMap, fmt::Debug};
use tracing::*;

const INDEX_ACCOUNT_HISTORY: StageId = StageId("IndexAccountHistoryStage");

/// Account hashing stage hashes plain account.
/// This is preparation before generating intermediate hashes and calculating Merkle tree root.
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
        INDEX_ACCOUNT_HISTORY
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let stage_progress = input.stage_progress.unwrap_or_default();
        let previous_stage_progress = input.previous_stage_progress();

        // read account changeset, merge it into one changeset and calculate account hashes.
        let from_transition = tx.get_block_transition(stage_progress)?;
        // NOTE: can probably done more probabilistic take of bundles with transition but it is
        // guess game for later. Transitions better reflect amount of work.
        let to_block =
            std::cmp::min(stage_progress + self.commit_threshold, previous_stage_progress);
        let to_transition = tx.get_block_transition(to_block)?;

        tx.cursor_read::<tables::AccountChangeSet>()?
            .walk(from_transition)?
            .take_while(|res| res.as_ref().map(|(k, _)| *k < to_transition).unwrap_or_default())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // fold all account to one set of changed accounts
            .fold(BTreeMap::new(), |mut accounts: BTreeMap<Address, Vec<u64>>, (index, account)| {
                accounts.entry(account.address).or_default().push(index);
                accounts
            })
            .into_iter()
            // insert indexes to AccontHistory.
            .try_for_each(|(address, mut indices)| -> Result<(), StageError> {
                // load last shard and check if it is full, remove last shard and append indices.
                let indices = if let Some((shard_key, list)) =
                    tx.get_account_history_biggest_sharded_index(address)?
                {
                    if list.len() >= NUM_OF_INDICES_IN_SHARD {
                        // if latest shard is full, just append new indices
                        indices
                    } else {
                        // delete old shard so new one can be inserted.
                        tx.delete::<tables::AccountHistory>(shard_key, None)?;
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
                        tx.put::<tables::AccountHistory>(ShardedKey::new(address, biggest_id), list)
                    },
                )?;
                // get
                Ok(())
            })?;

        info!(target: "sync::stages::index_account_history", "Stage finished");
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

        let mut cursor = tx.cursor_write::<tables::AccountHistory>()?;

        //<<DB as DatabaseGat>::TXMut as DbTxMutGAT>::CursorMut
        tx.cursor_read::<tables::AccountChangeSet>()?
            .walk(from_transition_rev)?
            .take_while(|res| res.as_ref().map(|(k, _)| *k < to_transition_rev).unwrap_or_default())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // reverse so we can get lowest transition id where we need to unwind account.
            .rev()
            // fold all account and get last transition index
            .fold(BTreeMap::new(), |mut accounts: BTreeMap<Address, u64>, (index, account)| {
                // we just need address and lowest transition id.
                accounts.insert(account.address, index);
                accounts
            })
            .into_iter()
            // try to unwind the index
            .try_for_each(|(address, rem_index)| -> Result<(), StageError> {
                let shard_part =
                    unwind_account_history_shards::<DB>(&mut cursor, address, rem_index)?;

                // check last shard_part, if present, items needs to be reinserted.
                if let Some(biggest_index) = shard_part.last() {
                    // there are items in list
                    tx.put::<tables::AccountHistory>(
                        ShardedKey::new(address, *biggest_index as u64),
                        TransitionList::new(shard_part)
                            .expect("There is at least one element in list and it is sorted."),
                    )?;
                }
                Ok(())
            })?;
        // from HistoryIndex higher than that number.
        Ok(UnwindOutput { stage_progress: input.unwind_to })
    }
}

/// Unwind all history shards. For boundary shard, remove it from database and
/// return last part of shard with still valid items. If all full shard were removed, return list
/// would be empty but this does not mean that there is none shard left but that there is not
/// splitted shards.
pub fn unwind_account_history_shards<DB: Database>(
    cursor: &mut <<DB as DatabaseGAT<'_>>::TXMut as DbTxMutGAT<'_>>::CursorMut<
        tables::AccountHistory,
    >,
    address: Address,
    transition_id: TransitionId,
) -> Result<Vec<usize>, StageError> {
    cursor.seek_exact(ShardedKey::new(address, u64::MAX))?;

    while let Some((sharded_key, list)) = cursor.prev()? {
        // there is no more shard for address
        if sharded_key.key != address {
            break
        }
        // check first item and if it is more and eq than `transition_id` delete current
        // item.
        let first = list.iter(0).next().expect("List can't empty");
        if first >= transition_id as usize {
            cursor.delete_current()?;
        } else if transition_id <= sharded_key.highest_transition_id {
            // if eq, last element needs to be removed.
            cursor.delete_current()?;

            // if first element is in scope whole list would be removed.
            // so at least this first element is present.
            let new_list =
                list.iter(0).take_while(|i| *i < transition_id as usize).collect::<Vec<_>>();
            return Ok(new_list)
        } else {
            return Ok(Vec::new())
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestTransaction, PREV_STAGE_ID};
    use reth_db::models::AccountBeforeTx;
    use reth_primitives::{hex_literal::hex, H160};

    const ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000001"));

    fn acc() -> AccountBeforeTx {
        AccountBeforeTx { address: ADDRESS, info: None }
    }

    /// Shard for account
    fn shard(shard_index: u64) -> ShardedKey<H160> {
        ShardedKey { key: ADDRESS, highest_transition_id: shard_index }
    }

    fn list(list: &[usize]) -> TransitionList {
        TransitionList::new(list).unwrap()
    }

    fn cast(
        table: Vec<(ShardedKey<H160>, TransitionList)>,
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
            tx.put::<tables::BlockTransitionIndex>(0, 3).unwrap();
            tx.put::<tables::BlockTransitionIndex>(5, 7).unwrap();

            // setup changeset that are going to be applied to history index
            tx.put::<tables::AccountChangeSet>(4, acc()).unwrap();
            tx.put::<tables::AccountChangeSet>(6, acc()).unwrap();
            Ok(())
        })
        .unwrap()
    }

    async fn run(tx: &TestTransaction, run_to: u64) {
        let mut input = ExecInput::default();
        input.previous_stage = Some((PREV_STAGE_ID, run_to));
        let mut stage = IndexAccountHistoryStage::default();
        let mut tx = tx.inner();
        let out = stage.execute(&mut tx, input).await.unwrap();
        assert_eq!(out, ExecOutput { stage_progress: 5, done: true });
        tx.commit().unwrap();
    }

    async fn unwind(tx: &TestTransaction, unwind_from: u64, unwind_to: u64) {
        let mut input = UnwindInput::default();
        input.stage_progress = unwind_from;
        input.unwind_to = unwind_to;
        let mut stage = IndexAccountHistoryStage::default();
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
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(6), vec![4, 6]),]));

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
            tx.put::<tables::AccountHistory>(shard(3), list(&[1, 2, 3])).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(6), vec![1, 2, 3, 4, 6]),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(3), vec![1, 2, 3]),]));
    }

    #[tokio::test]
    async fn insert_index_to_full_shard() {
        // init
        let tx = TestTransaction::default();
        let full_list = vec![3; NUM_OF_INDICES_IN_SHARD];

        // setup
        partial_setup(&tx);
        tx.commit(|tx| {
            tx.put::<tables::AccountHistory>(shard(3), list(&full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(3), full_list.clone()), (shard(6), vec![4, 6])]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
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
            tx.put::<tables::AccountHistory>(shard(3), list(&close_full_list)).unwrap();
            Ok(())
        })
        .unwrap();

        // run
        run(&tx, 5).await;

        // verify
        close_full_list.push(4);
        close_full_list.push(6);
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
        assert_eq!(table, BTreeMap::from([(shard(6), close_full_list.clone()),]));

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        close_full_list.pop();
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
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
            tx.put::<tables::AccountHistory>(shard(1), list(&close_full_list)).unwrap();
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
            BTreeMap::from([(shard(4), close_full_list.clone()), (shard(6), vec![6])])
        );

        // unwind
        unwind(&tx, 5, 0).await;

        // verify initial state
        close_full_list.pop();
        let table = cast(tx.table::<tables::AccountHistory>().unwrap());
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
            tx.put::<tables::AccountHistory>(shard(1), list(&full_list)).unwrap();
            tx.put::<tables::AccountHistory>(shard(2), list(&full_list)).unwrap();
            tx.put::<tables::AccountHistory>(shard(3), list(&[2, 3])).unwrap();
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
                (shard(6), vec![2, 3, 4, 6])
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
                (shard(3), vec![2, 3])
            ])
        );
    }
}
