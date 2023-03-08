use super::AccountProvider;
use crate::BlockHashProvider;
use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{
    Address, BlockHash, BlockNumber, Bytecode, StorageKey, StorageValue, H256, KECCAK_EMPTY, U256,
};

/// An abstraction for a type that provides state data.
#[auto_impl(&, Box)]
pub trait StateProvider: BlockHashProvider + AccountProvider + Send + Sync {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>>;

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>>;

    /// Get account code by its address.
    ///
    /// Returns `None` if the account doesn't exist or account is not a contract
    fn account_code(&self, addr: Address) -> Result<Option<Bytecode>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        let acc = match self.basic_account(addr)? {
            Some(acc) => acc,
            None => return Ok(None),
        };

        if let Some(code_hash) = acc.bytecode_hash {
            if code_hash == KECCAK_EMPTY {
                return Ok(None)
            }
            // Get the code from the code hash
            return self.bytecode_by_hash(code_hash)
        }

        // Return `None` if no code hash is set
        Ok(None)
    }

    /// Get account balance by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_balance(&self, addr: Address) -> Result<Option<U256>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.balance)),
            None => Ok(None),
        }
    }

    /// Get account nonce by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_nonce(&self, addr: Address) -> Result<Option<u64>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.nonce)),
            None => Ok(None),
        }
    }
}

/// Light wrapper that returns `StateProvider` implementations that correspond to the given
/// `BlockNumber` or the latest state.
pub trait StateProviderFactory: Send + Sync {
    /// History State provider.
    type HistorySP<'a>: StateProvider
    where
        Self: 'a;
    /// Latest state provider.
    type LatestSP<'a>: StateProvider
    where
        Self: 'a;

    /// Storage provider for latest block.
    fn latest(&self) -> Result<Self::LatestSP<'_>>;

    /// Returns a [StateProvider] indexed by the given block number.
    fn history_by_block_number(&self, block: BlockNumber) -> Result<Self::HistorySP<'_>>;

    /// Returns a [StateProvider] indexed by the given block hash.
    fn history_by_block_hash(&self, block: BlockHash) -> Result<Self::HistorySP<'_>>;
}
