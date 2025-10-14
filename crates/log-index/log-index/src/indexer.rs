use alloy_primitives::{map::HashMap, B256};
use reth_log_index_common::{
    CompletedMap, FilterError, FilterMapColumns, FilterMapParams, LogValueIndex, MapIndex,
    MapRowIndex, ProcessBatchResult, RowCell, MAX_LAYERS,
};

/// Type alias for count of filled entries in a row
pub type Count = u32;

// keep the cache bounded per map
const ROW_CACHE_CAP: usize = 64 * 1024;

/// Incrementally indexes log values into filter maps, maintaining state.
#[derive(Debug, Clone)]
pub struct LogIndexer {
    /// Current log value index
    index: LogValueIndex,
    /// Filter map parameters
    params: FilterMapParams,
    /// Fill levels per layer. layer -> `row_idx` -> count
    fills: Vec<HashMap<u32, Count>>,
    /// Cache for row calculation. (layer, value) -> `row_idx`
    row_cache: HashMap<(u8, B256), u32>,
    /// Current map we're indexing
    current_map: MapIndex,
    /// The log value index where the current map started
    current_map_start_index: LogValueIndex,
    /// Accumulated pending rows for the current map
    pending_rows: HashMap<MapRowIndex, FilterMapColumns>,
    /// Track the last completed map index (for metadata)
    last_completed_map: Option<MapIndex>,
    /// Track the last log value index of the last completed map
    last_completed_log_value_index: Option<LogValueIndex>,
}

impl LogIndexer {
    /// Create a new `LogIndexer` starting from the given index
    pub fn new(starting_index: LogValueIndex, params: FilterMapParams) -> Self {
        let current_map = (starting_index >> params.log_values_per_map) as u32;
        let current_map_start_index = (current_map as u64) << params.log_values_per_map;

        Self {
            index: starting_index,
            params,
            fills: (0..MAX_LAYERS).map(|_| HashMap::default()).collect(),
            row_cache: HashMap::default(),
            current_map,
            current_map_start_index,
            pending_rows: HashMap::default(),
            last_completed_map: None,
            last_completed_log_value_index: None,
        }
    }

    /// Create a `LogIndexer` resuming from existing state (checkpoint recovery)
    pub fn from_checkpoint(
        starting_index: LogValueIndex,
        params: FilterMapParams,
        pending_rows: HashMap<MapRowIndex, FilterMapColumns>,
    ) -> Self {
        let current_map = (starting_index >> params.log_values_per_map) as u32;
        let current_map_start_index = (current_map as u64) << params.log_values_per_map;

        // Rebuild fills from pending_rows
        let mut fills: Vec<HashMap<u32, Count>> =
            (0..MAX_LAYERS).map(|_| HashMap::default()).collect();

        for (map_row_index, columns) in &pending_rows {
            // Extract row_index from map_row_index using params method
            let row_index = params.extract_row_index(*map_row_index, current_map);

            // Determine which layer this row belongs to
            let layer = params.determine_layer(columns.indices.len());

            // Update fill level
            fills[layer].insert(row_index, columns.indices.len() as u32);
        }

        // If we're resuming a map, the last completed map is the previous one
        let (last_completed_map, last_completed_log_value_index) = if current_map > 0 {
            (Some(current_map - 1), Some(current_map_start_index.saturating_sub(1)))
        } else {
            (None, None)
        };

        Self {
            index: starting_index,
            params,
            fills,
            row_cache: HashMap::default(),
            current_map,
            current_map_start_index,
            pending_rows,
            last_completed_map,
            last_completed_log_value_index,
        }
    }

    /// Process a batch of log values (from receipts)
    /// Returns information about completed maps
    pub fn process_batch(
        &mut self,
        log_values: impl Iterator<Item = B256>,
    ) -> Result<ProcessBatchResult, FilterError> {
        let mut completed_maps = Vec::new();
        let mut values_processed = 0;

        for value in log_values {
            // Check if we've moved to a new map
            let new_map = (self.index >> self.params.log_values_per_map) as u32;
            if new_map != self.current_map {
                // Map completed - create CompletedMap info
                let completed_map = CompletedMap {
                    map_index: self.current_map,
                    start_log_value_index: self.current_map_start_index,
                    end_log_value_index: self.index.saturating_sub(1), /* Last index of completed
                                                                        * map */
                    rows: self.pending_rows.drain().collect(),
                };

                // Update tracking
                self.last_completed_map = Some(self.current_map);
                self.last_completed_log_value_index = Some(completed_map.end_log_value_index);

                completed_maps.push(completed_map);

                // Clear state for new map
                self.clear_state();
                self.current_map = new_map;
                self.current_map_start_index = (new_map as u64) << self.params.log_values_per_map;
            }

            // Process single value
            let cell = self.process_single_value(&value)?;

            // Accumulate in pending_rows
            self.pending_rows
                .entry(cell.map_row_index)
                .or_default()
                .indices
                .push(cell.column_index);

            // Increment index
            self.index = self.index.saturating_add(1);
            values_processed += 1;
        }

        if completed_maps.is_empty() {
            Ok(ProcessBatchResult::NoMapsCompleted { values_processed })
        } else {
            Ok(ProcessBatchResult::MapsCompleted { completed_maps, values_processed })
        }
    }

    /// Process a single log value
    fn process_single_value(&mut self, value: &B256) -> Result<RowCell, FilterError> {
        let map = self.current_map;

        for layer in 0..MAX_LAYERS {
            // Row index calculation with caching
            let cache_key = (layer, *value);
            let row_index = if let Some(&row_index) = self.row_cache.get(&cache_key) {
                row_index
            } else {
                let row_index = self.params.row_index(map, layer as u32, value);
                if self.row_cache.len() < ROW_CACHE_CAP {
                    self.row_cache.insert(cache_key, row_index);
                }
                row_index
            };

            // Check fill level
            let fill_level = *self.fills[layer as usize].get(&row_index).unwrap_or(&0);
            if fill_level < self.params.max_row_length(layer as u32) {
                // Calculate indices
                let map_row_index = self.params.map_row_index(map, row_index);
                let column_index = self.params.column_index(self.index, value);

                // Update fill level
                self.fills[layer as usize].insert(row_index, fill_level + 1);

                // Return the cell
                return Ok(RowCell { map, map_row_index, column_index, index: self.index });
            }
        }

        // All layers full
        Err(FilterError::MaxLayersExceeded(MAX_LAYERS))
    }

    /// Clear state when moving to a new map
    fn clear_state(&mut self) {
        for fill in &mut self.fills {
            fill.clear();
        }
        self.row_cache.clear();
    }

    /// Get the current map index
    pub const fn current_map(&self) -> MapIndex {
        self.current_map
    }

    /// Get the current log value index
    pub const fn current_index(&self) -> LogValueIndex {
        self.index
    }

    /// Get the last completed map index
    pub const fn last_completed_map(&self) -> Option<MapIndex> {
        self.last_completed_map
    }

    /// Get the last log value index of the last completed map
    pub const fn last_completed_log_value_index(&self) -> Option<LogValueIndex> {
        self.last_completed_log_value_index
    }

    /// Get pending rows
    pub const fn pending_rows(&self) -> &HashMap<MapRowIndex, FilterMapColumns> {
        &self.pending_rows
    }

    /// Take pending rows
    pub fn take_pending_rows(&mut self) -> HashMap<MapRowIndex, FilterMapColumns> {
        std::mem::take(&mut self.pending_rows)
    }

    /// Get the cutoff log value index for completed maps
    /// This is useful for determining which block boundaries can be written
    pub const fn completed_maps_cutoff(&self) -> LogValueIndex {
        self.current_map_start_index
    }
}
