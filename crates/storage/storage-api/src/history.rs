use alloy_primitives::{Address, BlockNumber, B256};
use auto_impl::auto_impl;
use core::ops::{RangeBounds, RangeInclusive};
use reth_db_api::{cursor::DbCursorRO, models::BlockNumberAddress, table::Table, BlockNumberList};
use reth_db_models::AccountBeforeTx;
use reth_primitives_traits::StorageEntry;
use reth_storage_errors::provider::ProviderResult;

/// Location of a historical account or storage value.
#[derive(Debug, Eq, PartialEq)]
pub enum HistoryInfo {
    /// The key had not been written at the requested block.
    NotYetWritten,
    /// The value is in this block's changeset.
    InChangeset(BlockNumber),
    /// The value is in plain state.
    InPlainState,
    /// Pruning requires a plain-state fallback.
    MaybeInPlainState,
}

impl HistoryInfo {
    /// Resolves a history-index lookup to its storage location.
    pub const fn from_lookup(
        found_block: Option<BlockNumber>,
        is_before_first_write: bool,
        lowest_available: Option<BlockNumber>,
    ) -> Self {
        if is_before_first_write {
            if let (Some(_), Some(block_number)) = (lowest_available, found_block) {
                return Self::InChangeset(block_number)
            }
            return Self::NotYetWritten
        }

        match found_block {
            Some(block_number) => Self::InChangeset(block_number),
            None => Self::InPlainState,
        }
    }
}

/// Reads account and storage history indices.
#[auto_impl(&, Arc, Box)]
pub trait HistoryReader: Send {
    /// Looks up an account's historical storage location.
    fn account_history_info(
        &self,
        address: Address,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo>;

    /// Looks up a storage slot's historical storage location.
    fn storage_history_info(
        &self,
        address: Address,
        storage_key: B256,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo>;
}

/// Computes the position of the first history entry at or after `block_number`.
#[inline]
pub fn compute_history_rank(
    chunk: &BlockNumberList,
    block_number: BlockNumber,
) -> (u64, Option<u64>) {
    let mut rank = chunk.rank(block_number);
    if rank.checked_sub(1).and_then(|rank| chunk.select(rank)) == Some(block_number) {
        rank -= 1;
    }
    (rank, chunk.select(rank))
}

/// Returns whether a history lookup needs to inspect the preceding shard.
#[inline]
pub fn needs_prev_shard_check(
    rank: u64,
    found_block: Option<BlockNumber>,
    block_number: BlockNumber,
) -> bool {
    rank == 0 && found_block != Some(block_number)
}

/// Looks up a key in a sharded history table.
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
    if let Some(chunk) = cursor.seek(key)?.filter(|(key, _)| key_filter(key)).map(|entry| entry.1) {
        let (rank, found_block) = compute_history_rank(&chunk, block_number);
        let is_before_first_write = needs_prev_shard_check(rank, found_block, block_number) &&
            !cursor.prev()?.is_some_and(|(key, _)| key_filter(&key));
        Ok(HistoryInfo::from_lookup(
            found_block,
            is_before_first_write,
            lowest_available_block_number,
        ))
    } else if lowest_available_block_number.is_some() {
        Ok(HistoryInfo::MaybeInPlainState)
    } else {
        Ok(HistoryInfo::NotYetWritten)
    }
}

/// History Writer
#[auto_impl(&, Arc, Box)]
pub trait HistoryWriter: Send {
    /// Unwind and clear account history indices.
    ///
    /// Returns number of changesets walked.
    fn unwind_account_history_indices<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx)>,
    ) -> ProviderResult<usize>;

    /// Unwind and clear account history indices in a given block range.
    ///
    /// Returns number of changesets walked.
    fn unwind_account_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize>;

    /// Insert account change index to database. Used inside `AccountHistoryIndex` stage
    fn insert_account_history_index(
        &self,
        index_updates: impl IntoIterator<Item = (Address, impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()>;

    /// Unwind and clear storage history indices.
    ///
    /// Returns number of changesets walked.
    fn unwind_storage_history_indices(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<usize>;

    /// Unwind and clear storage history indices in a given block range.
    ///
    /// Returns number of changesets walked.
    fn unwind_storage_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize>;

    /// Insert storage change index to database. Used inside `StorageHistoryIndex` stage
    fn insert_storage_history_index(
        &self,
        storage_transitions: impl IntoIterator<Item = ((Address, B256), impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()>;

    /// Read account/storage changesets and update account/storage history indices.
    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()>;
}
