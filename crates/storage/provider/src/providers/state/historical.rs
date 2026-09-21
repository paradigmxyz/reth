use alloy_primitives::BlockNumber;
use reth_db_api::{cursor::DbCursorRO, table::Table, BlockNumberList};
use reth_storage_api::HistoryInfo as ReaderHistoryInfo;
use reth_storage_errors::provider::ProviderResult;

/// Result of a history lookup for an account or storage slot.
///
/// Indicates where to find the historical value for a given key at a specific block.
#[derive(Debug, Eq, PartialEq)]
pub enum HistoryInfo {
    /// The key is written to, but only after our block (not yet written at the target block). Or
    /// it has never been written.
    NotYetWritten,
    /// The chunk contains an entry for a write after our block at the given block number.
    /// The value should be looked up in the changeset at this block.
    InChangeset(u64),
    /// The chunk does not contain an entry for a write after our block. This can only
    /// happen if this is the last chunk, so we need to look in the plain state.
    InPlainState,
    /// The key may have been written, but due to pruning we may not have changesets and
    /// history, so we need to make a plain state lookup.
    MaybeInPlainState,
}

impl HistoryInfo {
    /// Determines where to find the historical value based on computed shard lookup results.
    ///
    /// This is a pure function shared by both MDBX and `RocksDB` backends.
    ///
    /// # Arguments
    /// * `found_block` - The block number from the shard lookup
    /// * `is_before_first_write` - True if the target block is before the first write to this key.
    ///   This should be computed as: `rank == 0 && found_block != Some(block_number) &&
    ///   !has_previous_shard` where `has_previous_shard` comes from a lazy `cursor.prev()` check.
    /// * `lowest_available` - Lowest block where history is available (pruning boundary)
    pub const fn from_lookup(
        found_block: Option<u64>,
        is_before_first_write: bool,
        lowest_available: Option<BlockNumber>,
    ) -> Self {
        if is_before_first_write {
            if let (Some(_), Some(block_number)) = (lowest_available, found_block) {
                // The key may have been written, but due to pruning we may not have changesets
                // and history, so we need to make a changeset lookup.
                return Self::InChangeset(block_number)
            }
            // The key is written to, but only after our block.
            return Self::NotYetWritten
        }

        if let Some(block_number) = found_block {
            // The chunk contains an entry for a write after our block, return it.
            Self::InChangeset(block_number)
        } else {
            // The chunk does not contain an entry for a write after our block. This can only
            // happen if this is the last chunk and so we need to look in the plain state.
            Self::InPlainState
        }
    }
}

impl From<ReaderHistoryInfo> for HistoryInfo {
    fn from(info: ReaderHistoryInfo) -> Self {
        match info {
            ReaderHistoryInfo::NotYetWritten => Self::NotYetWritten,
            ReaderHistoryInfo::InChangeset(block_number) => Self::InChangeset(block_number),
            ReaderHistoryInfo::InPlainState => Self::InPlainState,
            ReaderHistoryInfo::MaybeInPlainState => Self::MaybeInPlainState,
        }
    }
}

impl From<HistoryInfo> for ReaderHistoryInfo {
    fn from(info: HistoryInfo) -> Self {
        match info {
            HistoryInfo::NotYetWritten => Self::NotYetWritten,
            HistoryInfo::InChangeset(block_number) => Self::InChangeset(block_number),
            HistoryInfo::InPlainState => Self::InPlainState,
            HistoryInfo::MaybeInPlainState => Self::MaybeInPlainState,
        }
    }
}

/// Computes the rank and finds the next modification block in a history shard.
///
/// Given a `block_number`, this function returns:
/// - `rank`: The number of entries strictly before `block_number` in the shard
/// - `found_block`: The block number at position `rank` (i.e., the first block >= `block_number`
///   where a modification occurred), or `None` if `rank` is out of bounds
///
/// The rank is adjusted when `block_number` exactly matches an entry in the shard,
/// so that `found_block` always returns the modification at or after the target.
///
/// This logic is shared between MDBX cursor-based lookups and `RocksDB` iterator lookups.
#[inline]
pub fn compute_history_rank(
    chunk: &BlockNumberList,
    block_number: BlockNumber,
) -> (u64, Option<u64>) {
    let mut rank = chunk.rank(block_number);
    // `rank(block_number)` returns count of entries <= block_number.
    // We want the first entry >= block_number, so if block_number is in the shard,
    // we need to step back one position to point at it (not past it).
    if rank.checked_sub(1).and_then(|r| chunk.select(r)) == Some(block_number) {
        rank -= 1;
    }
    (rank, chunk.select(rank))
}

/// Checks if a previous shard lookup is needed to determine if we're before the first write.
///
/// Returns `true` when `rank == 0` (first entry in shard) and the found block doesn't match
/// the target block number. In this case, we need to check if there's a previous shard.
#[inline]
pub fn needs_prev_shard_check(
    rank: u64,
    found_block: Option<u64>,
    block_number: BlockNumber,
) -> bool {
    rank == 0 && found_block != Some(block_number)
}

/// Generic history lookup for sharded history tables.
///
/// Seeks to the shard containing `block_number`, verifies the key via `key_filter`,
/// and checks previous shard to detect if we're before the first write.
pub fn history_info<T, K, C>(
    cursor: &mut C,
    key: K,
    block_number: BlockNumber,
    key_filter: impl Fn(&K) -> bool,
    lowest_available_block_number: Option<BlockNumber>,
) -> ProviderResult<HistoryInfo>
where
    T: Table<Key = K, Value = BlockNumberList>,
    C: DbCursorRO<T>,
{
    // Lookup the history chunk in the history index. If the key does not appear in the
    // index, the first chunk for the next key will be returned so we filter out chunks that
    // have a different key.
    if let Some(chunk) = cursor.seek(key)?.filter(|(k, _)| key_filter(k)).map(|x| x.1) {
        let (rank, found_block) = compute_history_rank(&chunk, block_number);

        // If our block is before the first entry in the index chunk and this first entry
        // doesn't equal to our block, it might be before the first write ever. To check, we
        // look at the previous entry and check if the key is the same.
        // This check is worth it, the `cursor.prev()` check is rarely triggered (the if will
        // short-circuit) and when it passes we save a full seek into the changeset/plain state
        // table.
        let is_before_first_write = needs_prev_shard_check(rank, found_block, block_number) &&
            !cursor.prev()?.is_some_and(|(k, _)| key_filter(&k));

        Ok(HistoryInfo::from_lookup(
            found_block,
            is_before_first_write,
            lowest_available_block_number,
        ))
    } else if lowest_available_block_number.is_some() {
        // The key may have been written, but due to pruning we may not have changesets and
        // history, so we need to make a plain state lookup.
        Ok(HistoryInfo::MaybeInPlainState)
    } else {
        // The key has not been written to at all.
        Ok(HistoryInfo::NotYetWritten)
    }
}

#[cfg(test)]
mod tests {
    use super::{needs_prev_shard_check, HistoryInfo};

    #[test]
    fn test_history_info_from_lookup() {
        // Before first write, no pruning → not yet written
        assert_eq!(HistoryInfo::from_lookup(Some(10), true, None), HistoryInfo::NotYetWritten);
        assert_eq!(HistoryInfo::from_lookup(None, true, None), HistoryInfo::NotYetWritten);

        // Before first write WITH pruning → check changeset (pruning may have removed history)
        assert_eq!(HistoryInfo::from_lookup(Some(10), true, Some(5)), HistoryInfo::InChangeset(10));
        assert_eq!(HistoryInfo::from_lookup(None, true, Some(5)), HistoryInfo::NotYetWritten);

        // Not before first write → check changeset or plain state
        assert_eq!(HistoryInfo::from_lookup(Some(10), false, None), HistoryInfo::InChangeset(10));
        assert_eq!(HistoryInfo::from_lookup(None, false, None), HistoryInfo::InPlainState);
    }

    #[test]
    fn test_needs_prev_shard_check() {
        // Only needs check when rank == 0 and found_block != block_number
        assert!(needs_prev_shard_check(0, Some(10), 5));
        assert!(needs_prev_shard_check(0, None, 5));
        assert!(!needs_prev_shard_check(0, Some(5), 5)); // found_block == block_number
        assert!(!needs_prev_shard_check(1, Some(10), 5)); // rank > 0
    }
}
