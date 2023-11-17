use auto_impl::auto_impl;
use reth_db::models::AccountBeforeTx;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{Account, Address, BlockNumber};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{RangeBounds, RangeInclusive},
};

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountReader: Send + Sync {
    /// Get basic account information.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>>;
}

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountExtReader: Send + Sync {
    /// Iterate over account changesets and return all account address that were changed.
    fn changed_accounts_with_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
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

/// AccountChange reader
#[auto_impl(&, Arc, Box)]
pub trait ChangeSetReader: Send + Sync {
    /// Iterate over account changesets and return the account state from before this block.
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>>;
}
