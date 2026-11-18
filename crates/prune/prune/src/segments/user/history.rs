use crate::{segments::PruneInput, PruneLimiter, PrunerError};
use alloy_primitives::{map::HashMap, BlockNumber};
use itertools::Itertools;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    models::ShardedKey,
    table::{Table, TableRow},
    transaction::DbTxMut,
    BlockNumberList, DatabaseError, RawKey, RawTable, RawValue,
};
use reth_provider::DBProvider;
use reth_prune_types::{SegmentOutput, SegmentOutputCheckpoint};
use std::hash::Hash;

/// The first surviving shard for a key touched in this transaction.
struct RetainedShard {
    /// Highest block actually present, including for the sentinel shard.
    last_index: BlockNumber,
    /// Later changesets in this shard can share a single bitmap update at commit.
    pending_cutoff: Option<BlockNumber>,
}

/// The position reached while pruning a key's ordered shards.
enum ShardPruneProgress {
    /// The highest index in the first survivor, or no remaining shard for the key.
    Finished(Option<BlockNumber>),
    /// A deletion exhausted the budget before the changeset could be removed.
    Interrupted { removed_cutoff: bool },
}

/// Result of pruning history changesets, used to build the final output.
pub(crate) struct HistoryPruneResult<K> {
    /// Map of the highest deleted changeset keys to their block numbers.
    pub(crate) highest_deleted: HashMap<K, BlockNumber>,
    /// The highest block number whose changesets are fully pruned, becoming the checkpoint.
    ///
    /// Static file walks stop on block boundaries so the next run starts at the next block.
    pub(crate) last_pruned_block: Option<BlockNumber>,
    /// Number of changesets pruned.
    pub(crate) pruned_count: usize,
    /// Whether pruning is complete.
    pub(crate) done: bool,
}

/// Finalizes history pruning by sorting sharded keys, pruning history indices, and building output.
///
/// Static file changesets are scanned through a block boundary before pruning the MDBX indices.
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

    // Seek from each key's first shard, including shards strictly below the cutoff.
    let mut cursor = provider.tx_ref().cursor_write::<RawTable<T>>()?;
    let mut pruned = pruned_count;
    for (key, cutoff) in highest_deleted.into_iter().sorted_unstable() {
        prune_key::<T, _>(
            &mut cursor,
            &to_sharded_key(key, 0),
            cutoff.min(last_changeset_pruned_block),
            &key_matches,
            || {
                pruned += 1;
                false // The static file walk finalizes whole blocks.
            },
        )?;
    }

    let progress = limiter.progress(done);

    Ok(SegmentOutput {
        progress,
        pruned,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: Some(last_changeset_pruned_block),
            tx_number: None,
        }),
    })
}

/// Prunes MDBX changesets in block order. The current changeset stays on disk until its key's
/// expired shards have been removed, so it also identifies where to resume after a commit.
/// A checkpoint marks the highest block whose history may be incomplete. The first physical
/// row can be in that block, or even below an older checkpoint.
pub(super) fn prune_database_history<Changesets, History, SK>(
    tx: &impl DbTxMut,
    end_key: Changesets::Key,
    input: PruneInput,
    history_key: impl Fn(TableRow<Changesets>) -> (BlockNumber, History::Key),
    key_matches: impl Fn(&History::Key, &History::Key) -> bool,
) -> Result<SegmentOutput, PrunerError>
where
    Changesets: Table,
    History: Table<Value = BlockNumberList>,
    History::Key: AsRef<ShardedKey<SK>> + Hash,
{
    let previous = input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint);
    let mut limiter = input.limiter;
    if limiter.is_limit_reached() {
        return Ok(SegmentOutput::not_done(limiter.interrupt_reason(), previous))
    }

    let previous_block = previous.and_then(|checkpoint| checkpoint.block_number);
    let mut changesets = tx.cursor_write::<Changesets>()?;
    let mut walker = changesets.walk_range(..=end_key)?;
    let mut indices = tx.cursor_write::<RawTable<History>>()?;
    let mut retained = HashMap::<_, RetainedShard>::default();
    let mut pruned = 0;
    let mut last_pruned = None;

    let interrupted_block = loop {
        let Some(row) = walker.next() else { break None };
        let (block, first_shard) = history_key(row?);
        if limiter.is_limit_reached() {
            break Some(block)
        }

        // Coalesce trims of a hot key until the shard itself expires or the transaction ends.
        if let Some(shard) = retained.get_mut(&first_shard) &&
            block < shard.last_index
        {
            shard.pending_cutoff = Some(block);
        } else {
            match prune_key::<History, _>(&mut indices, &first_shard, block, &key_matches, || {
                pruned += 1;
                limiter.increment_deleted_entries_count();
                limiter.is_limit_reached()
            })? {
                ShardPruneProgress::Finished(Some(last_index)) => {
                    retained
                        .insert(first_shard, RetainedShard { last_index, pending_cutoff: None });
                }
                ShardPruneProgress::Finished(None) => {
                    retained.remove(&first_shard);
                }
                ShardPruneProgress::Interrupted { removed_cutoff } => {
                    if removed_cutoff {
                        last_pruned = Some(block);
                    }
                    break Some(block)
                }
            }
        }

        walker.delete_current()?;
        limiter.increment_deleted_entries_count();
        pruned += 1;
        last_pruned = Some(block);
    };

    let pending =
        retained.into_iter().filter_map(|(key, shard)| Some((key, shard.pending_cutoff?)));
    for (first_shard, cutoff) in pending.sorted_unstable() {
        // The shard has an index strictly above the cutoff, so this only updates its bitmap.
        prune_key::<History, _>(&mut indices, &first_shard, cutoff, &key_matches, || {
            unreachable!("a pending trim must retain its shard")
        })?;
    }

    // If pruning already changed this block, its parent may depend on a missing index or
    // changeset. Otherwise the next row and its history are still intact.
    let checkpoint = match interrupted_block {
        Some(_) if pruned == 0 => {
            return Ok(SegmentOutput::not_done(limiter.interrupt_reason(), previous))
        }
        Some(0) => None,
        Some(next) => Some(last_pruned.unwrap_or_default().max(next - 1)),
        None if pruned == 0 && previous_block >= Some(input.to_block) => {
            return Ok(SegmentOutput::done())
        }
        None => Some(last_pruned.unwrap_or(input.to_block)),
    };
    Ok(SegmentOutput {
        progress: limiter.progress(interrupted_block.is_none()),
        pruned,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: checkpoint.max(previous_block),
            tx_number: None,
        }),
    })
}

/// Prunes one key from its first shard through the inclusive cutoff. After each deletion,
/// `on_delete` reports whether to stop before advancing the cursor.
fn prune_key<T, SK>(
    cursor: &mut (impl DbCursorRO<RawTable<T>> + DbCursorRW<RawTable<T>>),
    first_shard: &T::Key,
    to_block: BlockNumber,
    key_matches: &impl Fn(&T::Key, &T::Key) -> bool,
    mut on_delete: impl FnMut() -> bool,
) -> Result<ShardPruneProgress, DatabaseError>
where
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<SK>>,
{
    let mut shard = cursor.seek(RawKey::new(first_shard.clone()))?;
    while let Some((raw_key, raw_blocks)) = shard {
        let key = raw_key.key()?;
        if !key_matches(&key, first_shard) {
            break
        }
        let highest = key.as_ref().highest_block_number;
        let removed_cutoff = if highest > to_block {
            let mut blocks = raw_blocks.value()?;
            let highest = blocks.max();
            if let Some(last_index) = highest &&
                last_index > to_block
            {
                if blocks.remove_range(..=to_block) > 0 {
                    cursor.upsert(raw_key, &RawValue::new(blocks))?;
                }
                return Ok(ShardPruneProgress::Finished(Some(last_index)))
            }
            highest == Some(to_block)
        } else {
            highest == to_block
        };

        cursor.delete_current()?;
        if on_delete() {
            return Ok(ShardPruneProgress::Interrupted { removed_cutoff })
        }
        shard = cursor.next()?;
    }
    Ok(ShardPruneProgress::Finished(None))
}
