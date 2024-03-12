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
pub(crate) fn prune_history_indices<DB, T, SK>(
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
