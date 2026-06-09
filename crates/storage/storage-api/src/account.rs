use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber, B256};
use auto_impl::auto_impl;
use core::ops::{RangeBounds, RangeInclusive};
use reth_db_models::AccountBeforeTx;
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountReader {
    /// Get basic account information.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>>;

    /// Get basic account information using a precomputed `keccak256(address)`.
    ///
    /// Callers that already have the hashed address (e.g. while building a `HashedPostState`) can
    /// use this to avoid re-hashing in providers that key state by hashed address (hashed-state /
    /// "v2" storage). `address` is still
    /// required for providers that key by plain address (plain storage, in-memory overlays) and for
    /// account caches.
    ///
    /// The default implementation ignores `hashed_address` and forwards to
    /// [`basic_account`](Self::basic_account), so it is always correct; only providers that key by
    /// hashed address override it to skip the redundant hash.
    fn basic_account_by_hashed(
        &self,
        address: &Address,
        hashed_address: B256,
    ) -> ProviderResult<Option<Account>> {
        let _ = hashed_address;
        self.basic_account(address)
    }
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
}
