//! Storage types for filter maps (EIP-7745).
//!
//! This module defines the storage representations of filter maps data
//! that can be stored in the database.

use crate::types::{BlockBoundary, FilterMapMeta, FilterResult};
use alloy_primitives::{BlockNumber, B256};
use std::ops::RangeBounds;

/// Rows for a specific map
#[derive(Debug, Clone)]
pub struct MapRow {
    pub map_row_index: u64,
    pub rows: Vec<u32>,
}

/// Rows for a specific map and value combination
#[derive(Debug, Clone)]
pub struct MapValueRows {
    /// The map index these rows are for
    pub map_index: u32,
    /// The value hash these rows are for
    pub value: B256,
    /// Rows across all layers for this (map, value) pair
    /// Ordered by layer (1..MAX_LAYERS)
    pub layers: Vec<Vec<u32>>,
}

/// Provider trait for reading filter map data.
#[auto_impl::auto_impl(&, Arc)]
pub trait LogIndexProvider: Send + Sync {
    /// Get filter map metadata.
    fn get_metadata(&self) -> FilterResult<Option<FilterMapMeta>>;

    /// Fetch **layer-0** rows for `value` across the inclusive map range `[map_start..=map_end]`.
    ///
    /// Preconditions:
    /// - `map_start <= map_end`
    /// - the range is contiguous, sorted, and **entirely within one epoch** (`map_start >>
    ///   log_maps_per_epoch == map_end >> log_maps_per_epoch`)
    ///
    /// Behavior:
    /// - The implementation must compute the epoch-scoped row index as `row_index(first_epoch_map,
    ///   0, value)` for that epoch and reuse it for all maps in the range.
    /// - Returns exactly `map_end - map_start + 1` vectors **in order**; an empty `Vec<u32>` means
    ///   “no row for that map”.
    fn get_base_layer_rows_for_value(
        &self,
        map_start: u32,
        map_end: u32,
        value: &B256,
    ) -> FilterResult<Vec<Vec<u32>>>;

    /// Fetch overflow rows for (map_index, value), starting at layer 1, inclusive.
    /// Continues layer-by-layer until a row with length < max_row_length(layer) or an empty row.
    /// Returns only layers >= 1 (layer 0 is *not* included).
    fn fetch_more_layers_for_map(
        &self,
        map_index: u32,
        value: &B256,
    ) -> FilterResult<Vec<Vec<u32>>>;

    /// Convenience: for (maps × values), fetch base band and, for any full base row,
    /// fetch more layers per map. Returns complete layered rows per (map,value).
    fn get_rows_until_short_row(
        &self,
        map_start: u32,
        map_end: u32,
        values: &[B256],
    ) -> FilterResult<Vec<MapValueRows>>;

    /// Get log value indices for a range of blocks.
    /// Returns ordered vec of (block_number, log_value_index).
    fn get_log_value_indices_range(
        &self,
        block_range: impl RangeBounds<BlockNumber>,
    ) -> FilterResult<Vec<BlockBoundary>>;
}
