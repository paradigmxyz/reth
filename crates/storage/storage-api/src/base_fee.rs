use crate::{StateProvider, StateProviderBox};

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, U256};
use reth_chainspec::EthChainSpec;
use reth_storage_errors::ProviderError;
use revm_database::{Database, State};

/// An instance of the trait can return the base fee for the next block.
pub trait BaseFeeProvider<P: StorageProvider> {
    /// Returns the base fee for the next block.
    fn next_block_base_fee<H: BlockHeader>(
        &self,
        provider: &mut P,
        parent_header: &H,
        ts: u64,
    ) -> Result<u64, P::Error>;
}

impl<T: EthChainSpec, P: StorageProvider> BaseFeeProvider<P> for T {
    fn next_block_base_fee<H: BlockHeader>(
        &self,
        _provider: &mut P,
        parent_header: &H,
        ts: u64,
    ) -> Result<u64, P::Error> {
        Ok(parent_header
            .next_block_base_fee(self.base_fee_params_at_timestamp(ts))
            .unwrap_or_default())
    }
}

/// A storage provider trait that can be implemented on foreign types.
pub trait StorageProvider {
    /// The error type.
    type Error;

    /// Returns the storage value at the address for the provided key.
    fn storage(&mut self, address: Address, key: U256) -> Result<U256, Self::Error>;
}

impl<DB: Database> StorageProvider for State<DB> {
    type Error = DB::Error;

    fn storage(&mut self, address: Address, key: U256) -> Result<U256, Self::Error> {
        let _ = self.load_cache_account(address)?;
        Database::storage(self, address, key)
    }
}

impl StorageProvider for StateProviderBox {
    type Error = ProviderError;

    fn storage(&mut self, address: Address, key: U256) -> Result<U256, Self::Error> {
        Ok(StateProvider::storage(self, address, key.into())?.unwrap_or_default())
    }
}
