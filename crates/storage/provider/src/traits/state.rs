use super::AccountProvider;
use crate::BlockHashProvider;
use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, H256};

/// An abstraction for a type that provides state data.
#[auto_impl(&)]
pub trait StateProvider: BlockHashProvider + AccountProvider + Send + Sync {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>>;

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>>;
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
