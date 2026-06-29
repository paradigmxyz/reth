use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber};
use auto_impl::auto_impl;
use core::ops::{Bound, RangeBounds, RangeInclusive};
use reth_db_models::AccountBeforeTx;
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;

use crate::StorageChangeSetReader;

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountReader {
    /// Get basic account information.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>>;
}

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountExtReader {
    /// Iterate over account changesets and return all account address that were changed.
    fn changed_accounts_with_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>>;

    /// Get basic account information for multiple accounts. A more efficient version than calling
    /// [`AccountReader::basic_account`] repeatedly.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_accounts(
        &self,
        _iter: impl IntoIterator<Item = Address>,
    ) -> ProviderResult<Vec<(Address, Option<Account>)>>;

    /// Iterate over account changesets and return all account addresses that were changed alongside
    /// each specific set of blocks.
    ///
    /// NOTE: Get inclusive range of blocks.
    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<BlockNumber>>>;
}

/// `AccountChange` reader
#[auto_impl(&, Arc, Box)]
pub trait ChangeSetReader {
    /// Iterate over account changesets and return the account state from before this block.
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>>;

    /// Search the block's changesets for the given address, and return the result.
    ///
    /// Returns `None` if the account was not changed in this block.
    fn get_account_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx>>;

    /// Get all account changesets in a range of blocks.
    ///
    /// Accepts any range type that implements `RangeBounds<BlockNumber>`, including:
    /// - `Range<BlockNumber>` (e.g., `0..100`)
    /// - `RangeInclusive<BlockNumber>` (e.g., `0..=99`)
    /// - `RangeFrom<BlockNumber>` (e.g., `0..`) - iterates until exhausted
    ///
    /// If there is no start bound, 0 is used as the start block.
    ///
    /// Returns a vector of (`block_number`, changeset) pairs.
    fn account_changesets_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx)>>;

    /// Returns the number of account changeset rows in the given block range without
    /// materializing the changesets.
    ///
    /// Accepts the same range types as [`Self::account_changesets_range`].
    fn count_account_changesets_in_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<u64>;
}

/// Converts a block range to an inclusive range for changeset counting.
///
/// Returns `None` if the range is empty.
pub fn inclusive_block_range(
    range: impl RangeBounds<BlockNumber>,
) -> Option<RangeInclusive<BlockNumber>> {
    let start = match range.start_bound() {
        Bound::Included(&n) => n,
        Bound::Excluded(&n) => n.saturating_add(1),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&n) => n,
        Bound::Excluded(&n) => n.saturating_sub(1),
        Bound::Unbounded => BlockNumber::MAX,
    };

    if start > end {
        return None
    }

    Some(start..=end)
}

/// Counts account and storage changeset rows in the given block range.
pub fn count_state_changes_in_range(
    provider: &(impl ChangeSetReader + StorageChangeSetReader),
    range: impl RangeBounds<BlockNumber>,
) -> ProviderResult<u64> {
    let Some(range) = inclusive_block_range(range) else {
        return Ok(0);
    };
    let account = provider.count_account_changesets_in_range(range.clone())?;
    let storage = provider.count_storage_changesets_in_range(range)?;
    Ok(account + storage)
}
