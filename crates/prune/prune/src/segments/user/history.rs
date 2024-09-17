use alloy_primitives::BlockNumber;
use reth_db::{BlockNumberList, RawKey, RawTable, RawValue};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::ShardedKey,
    table::Table,
    transaction::DbTxMut,
    DatabaseError,
};
use reth_provider::DBProvider;

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

/// Prune history indices according to the provided list of highest sharded keys.
///
/// Returns total number of deleted, updated and unchanged entities.
pub(crate) fn prune_history_indices<Provider, T, SK>(
    provider: &Provider,
    highest_sharded_keys: impl IntoIterator<Item = T::Key>,
    key_matches: impl Fn(&T::Key, &T::Key) -> bool,
) -> Result<PrunedIndices, DatabaseError>
where
    Provider: DBProvider<Tx: DbTxMut>,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    let mut outcomes = PrunedIndices::default();
    let mut cursor = provider.tx_ref().cursor_write::<RawTable<T>>()?;

    for sharded_key in highest_sharded_keys {
        // Seek to the shard that has the key >= the given sharded key
        // TODO: optimize
        let mut shard = cursor.seek(RawKey::new(sharded_key.clone()))?;

        // Get the highest block number that needs to be deleted for this sharded key
        let to_block = sharded_key.as_ref().highest_block_number;

        'shard: loop {
            let Some((key, block_nums)) =
                shard.map(|(k, v)| Result::<_, DatabaseError>::Ok((k.key()?, v))).transpose()?
            else {
                break
            };

            if key_matches(&key, &sharded_key) {
                match prune_shard(&mut cursor, key, block_nums, to_block, &key_matches)? {
                    PruneShardOutcome::Deleted => outcomes.deleted += 1,
                    PruneShardOutcome::Updated => outcomes.updated += 1,
                    PruneShardOutcome::Unchanged => outcomes.unchanged += 1,
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
                        cursor.upsert(RawKey::new(key), prev_value)?;
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
                RawValue::new(BlockNumberList::new_pre_sorted(higher_blocks)),
            )?;
            Ok(PruneShardOutcome::Updated)
        }
    }
}
