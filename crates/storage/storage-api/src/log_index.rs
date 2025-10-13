use alloc::vec::Vec;
use alloy_primitives::{BlockNumber, B256};
use core::ops::RangeBounds;
use reth_log_index_common::{BlockBoundary, FilterMapMeta};
use reth_storage_errors::provider::ProviderResult;

/// Provider trait for reading filter map data.
#[auto_impl::auto_impl(&, Arc)]
pub trait LogIndexProvider: Send + Sync {
    /// Get filter map metadata.
    fn get_metadata(&self) -> ProviderResult<Option<FilterMapMeta>>;

    /// Fetch rows for (map_index, value) per layer.
    fn get_rows_for_value_layer(
        &self,
        value: &B256,
        map_indices: &[u32],
        layer: u32,
    ) -> ProviderResult<Vec<(u32, Vec<u32>)>>;

    /// Get log value indices for a range of blocks.
    /// Returns ordered vec of (`block_number`, `log_value_index`).
    fn get_log_value_indices_range(
        &self,
        block_range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<BlockBoundary>>;
}
