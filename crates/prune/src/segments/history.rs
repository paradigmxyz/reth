use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    models::ShardedKey,
    table::Table,
    transaction::DbTxMut,
    BlockNumberList,
};
use reth_interfaces::db::DatabaseError;
use reth_primitives::BlockNumber;
use reth_provider::DatabaseProviderRW;

/// Prune history indices up to the provided block, inclusive.
///
/// Returns total number of processed (walked) and deleted entities.
pub(crate) fn prune_history_indices<DB, T, SK>(
    provider: &DatabaseProviderRW<DB>,
    to_block: BlockNumber,
    key_matches: impl Fn(&T::Key, &T::Key) -> bool,
    last_key: impl Fn(&T::Key) -> T::Key,
) -> Result<(usize, usize), DatabaseError>
where
    DB: Database,
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    let mut processed = 0;
    let mut deleted = 0;
    let mut cursor = provider.tx_ref().cursor_write::<T>()?;

    // Prune history table:
    // 1. If the shard has `highest_block_number` less than or equal to the target block number
    // for pruning, delete the shard completely.
    // 2. If the shard has `highest_block_number` greater than the target block number for
    // pruning, filter block numbers inside the shard which are less than the target
    // block number for pruning.
    while let Some(result) = cursor.next()? {
        let (key, blocks): (T::Key, BlockNumberList) = result;

        // If shard consists only of block numbers less than the target one, delete shard
        // completely.
        if key.as_ref().highest_block_number <= to_block {
            cursor.delete_current()?;
            deleted += 1;
            if key.as_ref().highest_block_number == to_block {
                // Shard contains only block numbers up to the target one, so we can skip to
                // the last shard for this key. It is guaranteed that further shards for this
                // sharded key will not contain the target block number, as it's in this shard.
                cursor.seek_exact(last_key(&key))?;
            }
        }
        // Shard contains block numbers that are higher than the target one, so we need to
        // filter it. It is guaranteed that further shards for this sharded key will not
        // contain the target block number, as it's in this shard.
        else {
            let new_blocks =
                blocks.iter(0).skip_while(|block| *block <= to_block as usize).collect::<Vec<_>>();

            // If there were blocks less than or equal to the target one
            // (so the shard has changed), update the shard.
            if blocks.len() != new_blocks.len() {
                // If there are no more blocks in this shard, we need to remove it, as empty
                // shards are not allowed.
                if new_blocks.is_empty() {
                    if key.as_ref().highest_block_number == u64::MAX {
                        let prev_row = cursor.prev()?;
                        match prev_row {
                            // If current shard is the last shard for the sharded key that
                            // has previous shards, replace it with the previous shard.
                            Some((prev_key, prev_value)) if key_matches(&prev_key, &key) => {
                                cursor.delete_current()?;
                                deleted += 1;
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
                                deleted += 1;
                            }
                        }
                    }
                    // If current shard is not the last shard for this sharded key,
                    // just delete it.
                    else {
                        cursor.delete_current()?;
                        deleted += 1;
                    }
                } else {
                    cursor.upsert(key.clone(), BlockNumberList::new_pre_sorted(new_blocks))?;
                }
            }

            // Jump to the last shard for this key, if current key isn't already the last shard.
            if key.as_ref().highest_block_number != u64::MAX {
                cursor.seek_exact(last_key(&key))?;
            }
        }

        processed += 1;
    }

    Ok((processed, deleted))
}
