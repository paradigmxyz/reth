use alloy_primitives::BlockNumber;
use reth_db_models::StoredBlockBodyIndices;
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

///  Client trait for fetching block body indices related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockBodyIndicesProvider: Send + Sync {
    /// Returns the block body indices with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>>;

    /// Returns the block body indices within the requested range matching number from storage.
    fn block_body_indices_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>>;
}
