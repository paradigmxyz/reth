//! Constants for `LogIndex`

/// Maximum number of layers allowed in filter maps.
/// This is a safety limit to prevent infinite loops in case of corrupted data.
pub const MAX_LAYERS: u8 = 16;
