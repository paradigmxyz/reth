use crate::PruneLimiter;
use alloy_primitives::BlockNumber;
use itertools::Itertools;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::ShardedKey,
    table::Table,
    transaction::DbTxMut,
    BlockNumberList, DatabaseError, RawKey, RawTable, RawValue,
};
use reth_provider::DBProvider;
use reth_prune_types::{SegmentOutput, SegmentOutputCheckpoint};
use rustc_hash::FxHashMap;

enum PruneShardOutcome {
    Deleted,
    Updated,
    Unchanged,
}

#[derive(Debug, Default)]
pub(crate) struct PrunedIndices {
    pub(crate) deleted: usize,
    pub(crate) updated: usize,
    pub(crate) unchanged: usize,
}

/// Result of pruning history changesets, used to build the final output.
pub(crate) struct HistoryPruneResult<K> {
    /// Map of the highest deleted changeset keys to their block numbers.
    pub(crate) highest_deleted: FxHashMap<K, BlockNumber>,
    /// The highest block number whose changesets are fully pruned, becoming the checkpoint.
    ///
    /// Checkpoints have block granularity, so a caller that can stop in the middle of a block
    /// must report the block before it, and prune the interrupted block again on the next run.
    pub(crate) last_pruned_block: Option<BlockNumber>,
    /// Number of changesets pruned.
    pub(crate) pruned_count: usize,
    /// Whether pruning is complete.
    pub(crate) done: bool,
}

/// Finalizes history pruning by sorting sharded keys, pruning history indices, and building output.
///
/// This is shared between static file and database pruning for both account and storage history.
pub(crate) fn finalize_history_prune<Provider, T, K, SK>(
    provider: &Provider,
    result: HistoryPruneResult<K>,
    range_end: BlockNumber,
    limiter: &PruneLimiter,
    to_sharded_key: impl Fn(K, BlockNumber) -> T::Key,
    key_matches: impl Fn(&T::Key, &T::Key) -> bool,
) -> Result<SegmentOutput, DatabaseError>
where
    Provider: DBProvider<Tx: DbTxMut>,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
    K: Ord,
{
    let HistoryPruneResult { highest_deleted, last_pruned_block, pruned_count, done } = result;

    // Nothing was pruned only when the range held no changesets at all, so the whole range is
    // done.
    let last_changeset_pruned_block = last_pruned_block.unwrap_or(range_end);

    // Sort the keys so the shard walk follows on-disk order.
    // We use `sorted_unstable` because no equal keys exist in the map.
    let prune_targets = highest_deleted.into_iter().sorted_unstable().map(|(key, block_number)| {
        (to_sharded_key(key, 0), block_number.min(last_changeset_pruned_block))
    });

    let outcomes = prune_history_indices::<Provider, T, _>(provider, prune_targets, key_matches)?;

    let progress = limiter.progress(done);

    Ok(SegmentOutput {
        progress,
        pruned: pruned_count + outcomes.deleted,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: Some(last_changeset_pruned_block),
            tx_number: None,
        }),
    })
}

/// Prune history indices according to the provided targets, each pairing a key's first shard with
/// the highest block number to remove for that key.
///
/// Returns total number of deleted, updated and unchanged entities.
pub(crate) fn prune_history_indices<Provider, T, SK>(
    provider: &Provider,
    prune_targets: impl IntoIterator<Item = (T::Key, BlockNumber)>,
    key_matches: impl Fn(&T::Key, &T::Key) -> bool,
) -> Result<PrunedIndices, DatabaseError>
where
    Provider: DBProvider<Tx: DbTxMut>,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    let mut outcomes = PrunedIndices::default();
    let mut cursor = provider.tx_ref().cursor_write::<RawTable<T>>()?;

    for (first_shard_key, to_block) in prune_targets {
        // Start at the key's first shard rather than at `to_block`: a shard trimmed by an earlier
        // run keeps its original, higher key, so seeking past it would orphan it permanently.
        let mut shard = cursor.seek(RawKey::new(first_shard_key.clone()))?;

        'shard: loop {
            let Some((key, block_nums)) =
                shard.map(|(k, v)| Result::<_, DatabaseError>::Ok((k.key()?, v))).transpose()?
            else {
                break
            };

            if key_matches(&key, &first_shard_key) {
                match prune_shard(&mut cursor, key, block_nums, to_block, &key_matches)? {
                    PruneShardOutcome::Deleted => outcomes.deleted += 1,
                    PruneShardOutcome::Updated => outcomes.updated += 1,
                    // Shards are ordered by their highest block number, so every later shard for
                    // this key holds only higher blocks and is unchanged too.
                    PruneShardOutcome::Unchanged => {
                        outcomes.unchanged += 1;
                        break 'shard
                    }
                }
            } else {
                // If such shard doesn't exist, skip to the next sharded key
                break 'shard
            }

            shard = cursor.next()?;
        }
    }

    Ok(outcomes)
}

/// Prunes one shard of a history table.
///
/// 1. If the shard has `highest_block_number` less than or equal to the target block number for
///    pruning, delete the shard completely.
/// 2. If the shard has `highest_block_number` greater than the target block number for pruning,
///    filter block numbers inside the shard which are less than the target block number for
///    pruning.
fn prune_shard<C, T, SK>(
    cursor: &mut C,
    key: T::Key,
    raw_blocks: RawValue<T::Value>,
    to_block: BlockNumber,
    key_matches: impl Fn(&T::Key, &T::Key) -> bool,
) -> Result<PruneShardOutcome, DatabaseError>
where
    C: DbCursorRO<RawTable<T>> + DbCursorRW<RawTable<T>>,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    // If shard consists only of block numbers less than the target one, delete shard
    // completely.
    if key.as_ref().highest_block_number <= to_block {
        cursor.delete_current()?;
        Ok(PruneShardOutcome::Deleted)
    }
    // Shard contains block numbers that are higher than the target one, so we need to
    // filter it. It is guaranteed that further shards for this sharded key will not
    // contain the target block number, as it's in this shard.
    else {
        let blocks = raw_blocks.value()?;
        let higher_blocks =
            blocks.iter().skip_while(|block| *block <= to_block).collect::<Vec<_>>();

        // If there were blocks less than or equal to the target one
        // (so the shard has changed), update the shard.
        if blocks.len() as usize == higher_blocks.len() {
            return Ok(PruneShardOutcome::Unchanged);
        }

        // If there will be no more blocks in the shard after pruning blocks below target
        // block, we need to remove it, as empty shards are not allowed.
        if higher_blocks.is_empty() {
            if key.as_ref().highest_block_number == u64::MAX {
                let prev_row = cursor
                    .prev()?
                    .map(|(k, v)| Result::<_, DatabaseError>::Ok((k.key()?, v)))
                    .transpose()?;
                match prev_row {
                    // If current shard is the last shard for the sharded key that
                    // has previous shards, replace it with the previous shard.
                    Some((prev_key, prev_value)) if key_matches(&prev_key, &key) => {
                        cursor.delete_current()?;
                        // Upsert will replace the last shard for this sharded key with
                        // the previous value.
                        cursor.upsert(RawKey::new(key), &prev_value)?;
                        Ok(PruneShardOutcome::Updated)
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
                        Ok(PruneShardOutcome::Deleted)
                    }
                }
            }
            // If current shard is not the last shard for this sharded key,
            // just delete it.
            else {
                cursor.delete_current()?;
                Ok(PruneShardOutcome::Deleted)
            }
        } else {
            cursor.upsert(
                RawKey::new(key),
                &RawValue::new(BlockNumberList::new_pre_sorted(higher_blocks)),
            )?;
            Ok(PruneShardOutcome::Updated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256};
    use reth_db_api::{models::storage_sharded_key::StorageShardedKey, tables, transaction::DbTx};
    use reth_provider::DatabaseProviderFactory;
    use reth_stages::test_utils::TestStageDB;

    fn storage_key_matches(a: &StorageShardedKey, b: &StorageShardedKey) -> bool {
        a.address == b.address && a.sharded_key.key == b.sharded_key.key
    }

    /// Two runs whose targets straddle a shard boundary. The first trims the lower shard without
    /// changing its key, so a walk starting at the second target would never see it again.
    #[test]
    fn prune_history_indices_revisits_shard_trimmed_by_earlier_run() {
        let db = TestStageDB::default();
        let address = Address::from([0x42; 20]);
        let storage_key = B256::from([0x01; 32]);

        let provider = db.factory.database_provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_write::<tables::StoragesHistory>().unwrap();
        cursor
            .upsert(
                StorageShardedKey::new(address, storage_key, 100),
                &BlockNumberList::new_pre_sorted([10, 50, 100]),
            )
            .unwrap();
        cursor
            .upsert(
                StorageShardedKey::last(address, storage_key),
                &BlockNumberList::new_pre_sorted([150, 200]),
            )
            .unwrap();
        drop(cursor);

        for to_block in [50, 150] {
            prune_history_indices::<_, tables::StoragesHistory, _>(
                &provider,
                [(StorageShardedKey::new(address, storage_key, 0), to_block)],
                storage_key_matches,
            )
            .unwrap();
        }

        // After the first run the lower shard holds only block 100, which the second run's target
        // of 150 covers, so nothing below the sentinel may survive.
        let remaining = provider
            .tx_ref()
            .cursor_read::<tables::StoragesHistory>()
            .unwrap()
            .walk(None)
            .unwrap()
            .map(|row| {
                let (key, list) = row.unwrap();
                (key.sharded_key.highest_block_number, list.iter().collect::<Vec<_>>())
            })
            .collect::<Vec<_>>();
        assert_eq!(remaining, vec![(u64::MAX, vec![200])]);
    }
}
