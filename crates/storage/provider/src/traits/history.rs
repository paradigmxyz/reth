use auto_impl::auto_impl;
use reth_db::models::BlockNumberAddress;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{Address, BlockNumber, B256};
use std::{
    collections::BTreeMap,
    ops::{Range, RangeInclusive},
};

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
        account_transitions: BTreeMap<Address, Vec<u64>>,
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
        storage_transitions: BTreeMap<(Address, B256), Vec<u64>>,
    ) -> ProviderResult<()>;

    /// Read account/storage changesets and update account/storage history indices.
    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()>;
}
