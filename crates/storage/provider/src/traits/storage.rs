use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Address, H256, U256};

/// Storage reader
#[auto_impl(&, Arc, Box)]
pub trait StorageReader: Send + Sync {
    /// Get plainstate storages from an Address.
    fn basic_storages(
        &self,
        iter: impl IntoIterator<Item = (Address, impl IntoIterator<Item = H256>)>,
    ) -> Result<Vec<(Address, Vec<(H256, U256)>)>>;
}
