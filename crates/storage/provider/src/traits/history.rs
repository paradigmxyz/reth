use alloy_primitives::{Address, BlockNumber, B256};
use auto_impl::auto_impl;
use reth_db_api::models::BlockNumberAddress;
use reth_storage_errors::provider::ProviderResult;
use std::ops::{Range, RangeInclusive};

/// History Writer
#[auto_impl(&, Arc, Box)]
pub trait HistoryWriter: Send + Sync {
    /// Unwind and clear account history indices.
    ///
    /// Returns number of changesets walked.
    fn unwind_account_history_indices(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<usize>;

    /// Insert account change index to database. Used inside AccountHistoryIndex stage
    fn insert_account_history_index(
        &self,
        index_updates: impl IntoIterator<Item = (Address, impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()>;

    /// Unwind and clear storage history indices.
    ///
    /// Returns number of changesets walked.
    fn unwind_storage_history_indices(
        &self,
        range: Range<BlockNumberAddress>,
    ) -> ProviderResult<usize>;

    /// Insert storage change index to database. Used inside StorageHistoryIndex stage
    fn insert_storage_history_index(
        &self,
        storage_transitions: impl IntoIterator<Item = ((Address, B256), impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()>;

    /// Read account/storage changesets and update account/storage history indices.
    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()>;
}
