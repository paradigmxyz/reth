use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Account, Address, Bytes, H256};

/// Account provider
#[auto_impl(&)]
pub trait AccountProvider: Send + Sync {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>>;

    /// Get account code by its address
    fn account_code(&self, address: Address) ->  Result<Option<Bytes>>;

    /// Get account code by its hash
    fn code_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>>;
}
