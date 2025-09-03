use alloy_primitives::{map::HashMap, B256};

use crate::{
    ColumnIndex, FilterError, FilterMapColumns, FilterMapParams, LogValueIndex, MapIndex,
    MapRowIndex, MAX_LAYERS,
};

/// Type alias for count of filled entries in a row
pub type Count = u32;

// keep the cache bounded per map
const ROW_CACHE_CAP: usize = 64 * 1024;

/// Represents a storage-ready log value with its position in the filter maps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCell {
    /// The map index this log value belongs to
    pub map: MapIndex,
    /// The row within the map
    pub map_row_index: MapRowIndex,
    /// The column within the row
    pub column_index: ColumnIndex,
    /// The global log value index
    pub index: LogValueIndex,
}

/// An iterator that yields (log_value_index, map_row_index, column_index) for each log value.
///
/// Tracks fill levels per map to handle overflow into higher layers.
/// Finds the correct row for each log value based on its index and value.
#[derive(Debug, Clone)]
pub struct LogValueIterator<I> {
    inner: I,
    index: LogValueIndex,                // current log value index
    params: FilterMapParams,             // filter map parameters
    fills: Vec<HashMap<u32, Count>>,     // fill levels per layer. [layer] row_idx -> count
    row_cache: HashMap<(u8, B256), u32>, // cache for row. (layer, value) -> row_idx
    log_values_mask: u64,                // mask for values per map
}

impl<I> LogValueIterator<I> {
    /// Create a new LogValueIterator from an inner iterator.
    pub fn new(inner: I, starting_index: u64, params: FilterMapParams) -> Self {
        let log_values_mask = params.values_per_map() - 1;
        Self {
            inner,
            index: starting_index,
            params,
            fills: (0..MAX_LAYERS).map(|_| HashMap::default()).collect(),
            row_cache: HashMap::default(),
            log_values_mask,
        }
    }

    /// Create a LogValueIterator resuming from existing state.
    /// This is useful for resuming indexing from an incomplete map.
    pub fn from_state(
        inner: I,
        starting_index: u64,
        params: FilterMapParams,
        pending_rows: &HashMap<MapRowIndex, FilterMapColumns>,
    ) -> Self {
        let log_values_mask = params.values_per_map() - 1;
        let current_map = (starting_index >> params.log_values_per_map) as u32;

        // Rebuild fills from pending_rows
        let mut fills: Vec<HashMap<u32, Count>> =
            (0..MAX_LAYERS).map(|_| HashMap::default()).collect();

        for (map_row_index, columns) in pending_rows {
            // Extract row_index from map_row_index using params method
            let row_index = params.extract_row_index(*map_row_index, current_map);

            // Determine which layer this row belongs to using params method
            let layer = params.determine_layer(columns.indices.len());

            // Update fill level
            fills[layer].insert(row_index, columns.indices.len() as u32);
        }

        Self {
            inner,
            index: starting_index,
            params,
            fills,
            row_cache: HashMap::default(),
            log_values_mask,
        }
    }

    /// Get the current map index based on the log value index.
    /// This is calculated as index >> log_values_per_map.
    #[inline]
    pub fn current_map(&self) -> u32 {
        (self.index >> self.params.log_values_per_map) as u32
    }

    /// Get the cutoff log value index for the current map.
    /// This is helpful for writing map boundaries.
    #[inline]
    pub fn cutoff_log_value_index(&self) -> u64 {
        (self.current_map() as u64) << self.params.log_values_per_map
    }

    /// Clear the fill levels for all layers.
    /// This is called when moving to a new map.
    pub fn clear_state(&mut self) {
        // Clear fills
        for fill in &mut self.fills {
            fill.clear();
        }
        // Clear row cache
        self.row_cache.clear();
    }
}

impl<I: Iterator<Item = B256>> Iterator for LogValueIterator<I> {
    type Item = Result<RowCell, FilterError>;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.inner.next()?;

        // If we've moved to a new map, clear state
        if self.index & self.log_values_mask == 0 {
            self.clear_state();
        }

        let map = self.current_map();
        for layer in 0..MAX_LAYERS {
            let cache_key = (layer, value);
            let row_index = if let Some(&row_index) = self.row_cache.get(&cache_key) {
                row_index
            } else {
                let row_index = self.params.row_index(map, layer as u32, &value);
                if self.row_cache.len() < ROW_CACHE_CAP {
                    self.row_cache.insert(cache_key, row_index);
                }
                row_index
            };

            let fill_level = *self.fills[layer as usize].get(&row_index).unwrap_or(&0);
            if fill_level < self.params.max_row_length(layer as u32) {
                let map_row_index = self.params.map_row_index(map, row_index);
                let column_index = self.params.column_index(self.index, &value);

                // Update fill level
                self.fills[layer as usize].insert(row_index, fill_level + 1);

                let cell = RowCell { map, map_row_index, column_index, index: self.index };

                // Increment the global log value index
                self.index = self.index.saturating_add(1);
                return Some(Ok(cell));
            }
        }
        Some(Err(FilterError::MaxLayersExceeded(MAX_LAYERS)))
    }
}
