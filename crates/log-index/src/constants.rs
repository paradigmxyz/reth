//! Constants for `FilterMaps` parameters

use crate::params::FilterMapParams;

/// Expected number of potential matches in typical queries.
/// Used as initial capacity to reduce allocations.
pub const EXPECTED_MATCHES: u8 = 8;

/// Maximum number of layers allowed in filter maps.
/// This is a safety limit to prevent infinite loops in case of corrupted data.
pub const MAX_LAYERS: u8 = 16;

/// Default parameters used on mainnet
pub const DEFAULT_PARAMS: FilterMapParams = FilterMapParams {
    log_map_height: 16,
    log_map_width: 24,
    log_maps_per_epoch: 10,
    log_values_per_map: 16,
    base_row_length_ratio: 8,
    log_layer_diff: 4,
};

/// Test parameters that put one log value per epoch, ensuring block exact tail unindexing for
/// testing
pub const RANGE_TEST_PARAMS: FilterMapParams = FilterMapParams {
    log_map_height: 4,
    log_map_width: 24,
    log_maps_per_epoch: 0,
    log_values_per_map: 0,
    base_row_length_ratio: 16, // baseRowLength >= 1
    log_layer_diff: 4,
};
