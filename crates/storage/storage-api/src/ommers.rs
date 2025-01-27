use crate::HeaderProvider;
use alloy_eips::BlockHashOrNumber;
use reth_storage_errors::provider::ProviderResult;

/// Client trait for fetching ommers.
pub trait OmmersProvider: HeaderProvider + Send + Sync {
    /// Returns the ommers/uncle headers of the given block from the database.
    ///
    /// Returns `None` if block is not found.
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Self::Header>>>;
}

impl<T: OmmersProvider> OmmersProvider for std::sync::Arc<T> {
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Self::Header>>> {
        T::ommers(self, id)
    }
}

impl<T: OmmersProvider> OmmersProvider for &T {
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Self::Header>>> {
        T::ommers(self, id)
    }
}
