use crate::Result;
use reth_primitives::{
    rpc::BlockId, Account, Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, H256,
    U256,
};

/// Provides access to storage data
pub trait StorageProvider: Send + Sync {
    /// Returns the value from a storage position at a given address and `BlockId`
    fn storage_at(&self, address: Address, index: U256, at: BlockId) -> Result<Option<H256>>;
}

/// Function needed for executor.
pub trait StateProvider: Send + Sync {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>>;

    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>>;

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>>;

    /// Get block hash by number.
    fn block_hash(&self, number: U256) -> Result<Option<H256>>;
}

/// Light wrapper that creates StateProvider.
pub trait StateProviderFactory: Send + Sync {
    /// History State provider.
    type HistorySP<'a>: StateProvider
    where
        Self: 'a;
    /// Latest state provider.
    type LatestSP<'a>: StateProvider
    where
        Self: 'a;
    /// Storage provider for latest block
    fn latest(&self) -> Result<Self::LatestSP<'_>>;

    /// History provider indexed by block number
    fn history_by_block_number(&self, block: BlockNumber) -> Result<Self::HistorySP<'_>>;

    /// History provider indexed by block hash
    fn history_by_block_hash(&self, block: BlockHash) -> Result<Self::HistorySP<'_>>;
}
