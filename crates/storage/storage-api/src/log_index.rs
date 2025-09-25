use alloc::vec::Vec;
use alloy_primitives::{BlockNumber, B256};
use core::ops::RangeBounds;
use reth_log_index_common::{BlockBoundary, FilterMapMeta, MapValueRows};
use reth_storage_errors::provider::ProviderResult;

/// Provider trait for reading filter map data.
#[auto_impl::auto_impl(&, Arc)]
pub trait LogIndexProvider: Send + Sync {
    /// Get filter map metadata.
    fn get_metadata(&self) -> ProviderResult<Option<FilterMapMeta>>;

    /// Fetch **layer-0** rows for `value` across the inclusive map range `[map_start..=map_end]`.
    fn get_base_layer_rows_for_value(
        &self,
        map_start: u32,
        map_end: u32,
        value: &B256,
    ) -> ProviderResult<Vec<Vec<u32>>>;

    /// Fetch overflow rows for (`map_index`, value), starting at layer 1, inclusive.
    /// Continues layer-by-layer until a row with length < `max_row_length(layer)` or an empty row.
    /// Returns only layers >= 1 (layer 0 is *not* included).
    fn fetch_more_layers_for_map(
        &self,
        map_index: u32,
        value: &B256,
    ) -> ProviderResult<Vec<Vec<u32>>>;

    /// Convenience: for (maps × values), fetch base band and, for any full base row,
    /// fetch more layers per map. Returns complete layered rows per (map,value).
    fn get_rows_until_short_row(
        &self,
        map_start: u32,
        map_end: u32,
        values: &[B256],
    ) -> ProviderResult<Vec<MapValueRows>>;

    /// Get log value indices for a range of blocks.
    /// Returns ordered vec of (`block_number`, `log_value_index`).
    fn get_log_value_indices_range(
        &self,
        block_range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<BlockBoundary>>;
}
