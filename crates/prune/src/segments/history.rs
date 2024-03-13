use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::ShardedKey,
    table::Table,
    transaction::DbTxMut,
    BlockNumberList,
};
use reth_interfaces::db::DatabaseError;
use reth_provider::{DatabaseProviderRW, PruneLimiter};
use tracing::trace;

/// Prune history indices up to the provided block, inclusive.
///
/// Returns total number of processed (walked) and deleted entities.
pub(crate) fn _prune_history_indices<DB, T, SK>(
    provider: &DatabaseProviderRW<DB>,
    to_block: u64,
    mut limiter: PruneLimiter,
    key_matches: impl Fn(&T::Key, &T::Key) -> bool,
    last_key: impl Fn(&T::Key) -> T::Key,
) -> Result<(usize, usize), DatabaseError>
where
    DB: Database,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    let mut processed = 0;
    let mut cursor = provider.tx_ref().cursor_write::<T>()?;

    // Prune history table:
    // 1. If the shard has `highest_block_number` less than or equal to the target block number
    // for pruning, delete the shard completely.
    // 2. If the shard has `highest_block_number` greater than the target block number for
    // pruning, filter block numbers inside the shard which are less than the target
    // block number for pruning.
    loop {
        // check for time out must be done in this scope since it's not done in
        // `step_prune_indices`
        if limiter.is_limit_reached() {
            break
        }

        match step_prune_indices::<DB, T, SK>(
            &mut cursor,
            to_block,
            &mut limiter,
            &key_matches,
            &last_key,
        )? {
            PruneStepResult::Finished | PruneStepResult::ReachedDeletedEntriesLimit => break,
            PruneStepResult::MaybeMoreData => processed += 1,
        }
    }

    Ok((processed, limiter.deleted_entries_count()))
}

/// Steps once with given cursor.
///
/// Caution! Prune job timeout is not checked, only limit on deleted entries count. This allows for
/// a clean exit of a prune job that's pruning different tables concurrently, by letting them step
/// to the same height before timing out.
pub(crate) fn step_prune_indices<DB, T, SK>(
    cursor: &mut <<DB as Database>::TXMut as DbTxMut>::CursorMut<T>,
    to_block: u64,
    limiter: &mut PruneLimiter,
    key_matches: &impl Fn(&T::Key, &T::Key) -> bool,
    last_key: &impl Fn(&T::Key) -> T::Key,
) -> Result<PruneStepResult, DatabaseError>
where
    DB: Database,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    trace!(target: "pruner::history", "pruning history");

    let Some((key, blocks)) = cursor.next()? else {
        trace!(target: "pruner::history",
            "cursor is at end of search, nothing more to prune",
        );

        return Ok(PruneStepResult::Finished)
    };

    trace!(target: "pruner::history", blocks_len=blocks.len(), "current shard");

    if let Some(limit) = limiter.deleted_entries_limit() {
        if limiter.deleted_entries_count() == limit {
            trace!(target: "pruner::history",
                "reached limit on deleted entries"
            );

            return Ok(PruneStepResult::ReachedDeletedEntriesLimit)
        }
    }

    // If shard consists only of block numbers less than the target one, delete shard
    // completely.
    if key.as_ref().highest_block_number <= to_block {
        cursor.delete_current()?;
        limiter.increment_deleted_entries_count();

        trace!(target: "pruner::history",
            "deleted complete shard"
        );

        if key.as_ref().highest_block_number == to_block {
            // Shard contains only block numbers up to the target one, so we can skip to
            // the last shard for this key. It is guaranteed that further shards for this
            // sharded key will not contain the target block number, as it's in this shard.
            cursor.seek_exact(last_key(&key))?;
        }
    } else {
        // Shard contains block numbers that are higher than the target one, so we need to
        // filter it. It is guaranteed that further shards for this sharded key will not
        // contain the target block number, as it's in this shard.
        let higher_blocks =
            blocks.iter().skip_while(|block| *block <= to_block).collect::<Vec<_>>();

        trace!(target: "pruner::history",
            to_block,
            highest_block_shard=key.as_ref().highest_block_number,
            higher_blocks_len=higher_blocks.len(),
            blocks_len=blocks.len(),
            "cannot prune complete shard"
        );

        // If there were blocks less than or equal to the target one
        // (so the shard has changed), update the shard.
        if blocks.len() as usize != higher_blocks.len() {
            // If there will be no more shards in the block after pruning blocks below target
            // block, we need to remove it, as empty shards are not allowed.
            if higher_blocks.is_empty() {
                if key.as_ref().highest_block_number == u64::MAX {
                    let prev_row = cursor.prev()?;
                    match prev_row {
                        // If current shard is the last shard for the sharded key that
                        // has previous shards, replace it with the previous shard.
                        Some((prev_key, prev_value)) if key_matches(&prev_key, &key) => {
                            cursor.delete_current()?;
                            limiter.increment_deleted_entries_count();

                            trace!(target: "pruner::history",
                                "deleted shard, shard is last shard of sharded key and has predecessors"
                            );

                            // Upsert will replace the last shard for this sharded key with
                            // the previous value.
                            cursor.upsert(key.clone(), prev_value)?;
                        }
                        // If there's no previous shard for this sharded key,
                        // just delete last shard completely.
                        _ => {
                            // If we successfully moved the cursor to a previous row,
                            // jump to the original last shard.
                            if prev_row.is_some() {
                                cursor.next()?;
                            }
                            // Delete shard.
                            cursor.delete_current()?;
                            limiter.increment_deleted_entries_count();

                            trace!(target: "pruner::history",
                                "deleted shard, shard is last and first shard of sharded key"
                            );
                        }
                    }
                }
                // If current shard is not the last shard for this sharded key,
                // just delete it.
                else {
                    cursor.delete_current()?;
                    limiter.increment_deleted_entries_count();

                    trace!(target: "pruner::history",
                        "deleted shard"
                    );
                }
            } else {
                cursor.upsert(key.clone(), BlockNumberList::new_pre_sorted(higher_blocks))?;
                limiter.increment_deleted_entries_count();

                trace!(target: "pruner::history",
                    "partially pruned shard"
                );
            }
        } else {
            trace!(target: "pruner::history",
                "shard unchanged, nothing to prune"
            )
        }
    }

    // Jump to the last shard for this key, if current key isn't already the last
    // shard.
    if key.as_ref().highest_block_number != u64::MAX {
        cursor.seek_exact(last_key(&key))?;
    }

    Ok(PruneStepResult::MaybeMoreData)
}

/// Result of stepping once with cursor and pruning destination.
#[derive(Debug)]
pub(crate) enum PruneStepResult {
    /// Cursor has no further destination.
    Finished,
    /// Limiter has interrupted search because the maximum number of entries for one prune job have
    /// been deleted.
    ReachedDeletedEntriesLimit,
    /// Data was pruned at current destination.
    MaybeMoreData,
}

#[cfg(test)]
pub(in crate::segments) mod test {
    use std::{
        collections::BTreeMap,
        ops::{Range, RangeInclusive},
    };

    use reth_db::{table::Table, tables};
    use reth_interfaces::test_utils::{
        generators,
        generators::{random_block_range, random_changeset_range, random_eoa_accounts},
    };
    use reth_primitives::{
        Account, Address, BlockNumber, IntegerList, PruneSegment, StorageEntry, B256,
    };
    use reth_provider::{PruneCheckpointReader, PruneLimiter};
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use tracing::trace;

    use crate::segments::PruneInput;

    pub(crate) type ChangeSetsList = Vec<Vec<(Address, Account, Vec<StorageEntry>)>>;

    pub(crate) type ShardsList<SK> = Vec<(SK, IntegerList)>;

    pub(crate) struct TestRig<SK> {
        db: TestStageDB,
        changesets: ChangeSetsList,
        original_shards: ShardsList<SK>,
        job_limiter: PruneLimiter,
        pruned_changesets_run_1: usize,
        pruned_changesets_run_2: usize,
        pruned_shards_run_1: usize,
        pruned_shards_run_2: usize,
    }

    impl<SK> TestRig<SK> {
        pub(crate) fn default_with_job_limiter(job_limiter: PruneLimiter) -> Self {
            Self::new(TestStageDB::default(), vec![], vec![], job_limiter)
        }

        pub(crate) fn new(
            db: TestStageDB,
            changesets: ChangeSetsList,
            original_shards: ShardsList<SK>,
            job_limiter: PruneLimiter,
        ) -> Self {
            Self {
                db,
                changesets,
                original_shards,
                job_limiter,
                pruned_changesets_run_1: 0,
                pruned_changesets_run_2: 0,
                pruned_shards_run_1: 0,
                pruned_shards_run_2: 0,
            }
        }

        pub(crate) fn db(&self) -> &TestStageDB {
            &self.db
        }

        pub(crate) fn job_limiter(&self) -> &PruneLimiter {
            &self.job_limiter
        }

        pub(crate) fn init_db(
            block_range: RangeInclusive<u64>,
            n_storage_changes: Range<u64>,
            key_range: Range<u64>,
        ) -> (TestStageDB, ChangeSetsList) {
            let db = TestStageDB::default();
            let mut rng = generators::rng();

            let blocks = random_block_range(&mut rng, block_range, B256::ZERO, 0..1);
            db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

            let accounts = random_eoa_accounts(&mut rng, 2).into_iter().collect::<BTreeMap<_, _>>();

            let (changesets, _) = random_changeset_range(
                &mut rng,
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                n_storage_changes,
                key_range,
            );
            db.insert_changesets(changesets.clone(), None).expect("insert changesets");
            db.insert_history(changesets.clone(), None).expect("insert history");

            (db, changesets)
        }

        pub(crate) fn get_input(
            &self,
            to_block: BlockNumber,
            prune_segment: PruneSegment,
            segment_limiter: PruneLimiter,
        ) -> PruneInput {
            PruneInput {
                previous_checkpoint: self
                    .db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(prune_segment)
                    .unwrap(),
                to_block,
                limiter: segment_limiter,
            }
        }

        pub(crate) fn pruned_changesets<T>(&mut self, run: usize) -> usize
        where
            T: Table,
            <T as Table>::Key: Default,
        {
            // change sets
            let changesets = self.db.table::<T>().unwrap();
            trace!(target: "pruner::test", original_changesets_len=self.changesets.len(), changesets_len=changesets.len());
            let pruned_changesets = self.changesets.len() - changesets.len();

            if run == 1 {
                self.pruned_changesets_run_1 = pruned_changesets;

                self.pruned_changesets_run_1
            } else if run == 2 {
                self.pruned_changesets_run_2 = pruned_changesets - self.pruned_changesets_run_1;

                self.pruned_changesets_run_2
            } else if run == 3 {
                pruned_changesets - self.pruned_changesets_run_1 - self.pruned_changesets_run_2
            } else {
                unreachable!()
            }
        }

        pub(crate) fn pruned_shards<T>(&mut self, run: usize) -> usize
        where
            T: Table<Value = IntegerList>,
            <T as Table>::Key: Default,
        {
            // shards
            let shards = self.db.table::<T>().unwrap();

            let completely_pruned_shards = self.original_shards.len() - shards.len();
            // branch not covered in test
            assert_eq!(0, completely_pruned_shards);

            let partially_pruned_shards = self.partially_pruned_shards::<tables::StoragesHistory>();

            if run == 1 {
                self.pruned_shards_run_1 = partially_pruned_shards;

                self.pruned_shards_run_1
            } else if run == 2 {
                self.pruned_shards_run_2 = partially_pruned_shards - self.pruned_shards_run_1;

                self.pruned_shards_run_2
            } else if run == 3 {
                partially_pruned_shards - self.pruned_shards_run_1 - self.pruned_shards_run_2
            } else {
                unreachable!()
            }
        }

        pub(crate) fn partially_pruned_shards<T>(&self) -> usize
        where
            T: Table<Value = IntegerList>,
            <T as Table>::Key: Default,
        {
            let mut partially_pruned_shards = 0;

            for (pruned_shard, original_shard) in
                self.db.table::<T>().unwrap().iter().zip(self.original_shards.iter())
            {
                let original_shard_blocks = &original_shard.1;
                let pruned_shard_blocks = &pruned_shard.1;
                if original_shard_blocks != pruned_shard_blocks {
                    let original_shard_blocks_len = original_shard_blocks.len() as usize;
                    let pruned_shard_blocks_len = pruned_shard_blocks.len() as usize;

                    trace!(target: "pruner::test",
                        original_shard_blocks_len,
                        pruned_shard_blocks_len,
                        "partially pruned shard"
                    );

                    // since each step in pruning indices, if partially pruning the shard, makes a
                    // new shard (upserts) with at most one block less than the current one, the
                    // number of partially pruned shards is equal to the number of blocks pruned
                    // from that shard
                    partially_pruned_shards += original_shard_blocks_len - pruned_shard_blocks_len;
                }
            }

            partially_pruned_shards
        }
    }
}
