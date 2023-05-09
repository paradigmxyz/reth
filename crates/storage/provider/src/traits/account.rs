use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Account, Address};

/// Account provider
#[auto_impl(&, Arc, Box)]
pub trait AccountProvider: Send + Sync {
    /// Get basic account information.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_account(&self, address: Address) -> Result<Option<Account>>;
}
