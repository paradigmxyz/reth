use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Account, Address, BlockNumber};
use std::{collections::BTreeSet, ops::RangeBounds};

/// Account provider
#[auto_impl(&, Arc, Box)]
pub trait AccountProvider: Send + Sync {
    /// Get basic account information.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: Address) -> Result<Option<Account>>;
}

/// Account provider
#[auto_impl(&, Arc, Box)]
pub trait AccountExtProvider: Send + Sync {
    /// Iterate over account changesets and return all account address that were changed.
    fn changed_accounts_with_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<BTreeSet<Address>>;

    /// Get basic account information for multiple accounts. A more efficient version than calling
    /// [`AccountProvider::basic_account`] repeatedly.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_accounts(
        &self,
        _iter: impl IntoIterator<Item = Address>,
    ) -> Result<Vec<(Address, Option<Account>)>>;
}
