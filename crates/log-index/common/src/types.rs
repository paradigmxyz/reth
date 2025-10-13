//! Core types used by the `FilterMaps` implementation.

use alloc::{boxed::Box, string::String, vec::Vec};
use alloy_primitives::{BlockNumber, B256};
use thiserror::Error;

/// Type alias for Map Index
/// This represents the index of a filter map.
pub type MapIndex = u32;

/// Type alias for Map Row Index
/// This represents the index of a row in the filter map.
pub type MapRowIndex = u64;

/// Type alias for column index
/// This represents the index of a column in the filter map row.
pub type ColumnIndex = u32;

/// Type alias for Log Value Index
/// This represents the index of a log value in the log index.
pub type LogValueIndex = u64;

/// Metadata for tracking the state of log indexing and filter map generation.
///
/// This struct maintains information about which blocks have been indexed and which filter maps
/// have been generated. It is used to track progress and enable resuming of the indexing process.
#[derive(Debug, Clone, Default, PartialEq, Eq, Copy)]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FilterMapMeta {
    /// The first block number that has had its logs fully indexed.
    /// This represents the starting point of our complete log index.
    pub first_indexed_block: BlockNumber,

    /// The last block number that has had its logs fully indexed.
    /// This represents how far the log indexing has progressed.
    pub last_indexed_block: BlockNumber,

    /// Is the last indexed block's logs complete?
    /// TODO: need to impl this in the crate. This can be used to get the indexed range.
    pub is_last_indexed_block_complete: bool,

    /// The index of the first complete filter map that has been stored.
    /// Filter maps before this index may be incomplete or missing.
    pub first_map_index: u32,

    /// The index of the last complete filter map that has been stored.
    pub last_map_index: u32,

    /// The last indexed log value index.
    /// Used to resume log indexing from where it left off previously.
    pub last_log_value_index: u64,

    /// The number of maps in the oldest epoch. This is used to be pruning-aware.
    /// When pruning is enabled, we need to know how many maps were created in the oldest epoch
    /// that are still present in the db.
    ///
    /// TODO: need to implement pruning logic in the crate.
    pub oldest_epoch_map_count: u32,
}

/// A filter map row.
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterMapRow {
    /// The index of the filter map row.
    pub map_row_index: u64,
    /// The columns in the filter map.
    pub columns: FilterMapColumns,
}

/// Filter map column.
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterMapColumns {
    /// Indices of the columns in the filter map.
    pub indices: Vec<u32>,
}

/// Represents the block boundaries for log value indices.
///
/// Each entry indicates the starting log value index for a specific block number.
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::Compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct BlockBoundary {
    /// The block number.
    pub block_number: BlockNumber,
    /// The log value index of the first log entry in the block.
    pub log_value_index: u64,
}

/// Errors that can occur when using `FilterMaps`.
#[derive(Error, Debug)]
pub enum FilterError {
    /// No filter constraints provided.
    #[error("no filter constraints provided")]
    NoConstraints,
    /// Database error occurred.
    #[error("database error: {0}")]
    Database(String),

    /// Invalid base band configuration.
    #[error("get_base_layer_rows: maps must be sorted and within the same epoch")]
    InvalidBaseBand,

    /// The maps slice is empty.
    #[error("maps slice is empty")]
    EmptyMaps,

    /// Invalid block range specified.
    #[error("invalid block range: {0} > {1}")]
    InvalidRange(BlockNumber, BlockNumber),

    /// Insufficient layers in filter map row alternatives.
    #[error("insufficient filter map layers for map {0}")]
    InsufficientLayers(u32),

    /// Corrupted filter map data detected.
    #[error("corrupted filter map data: {0}")]
    CorruptedData(String),

    /// Maximum layer limit exceeded.
    #[error("maximum layer limit ({0}) exceeded")]
    MaxLayersExceeded(u8),

    /// Invalid filter map parameters.
    #[error("invalid filter map parameters: {0}")]
    InvalidParameters(String),

    /// Invalid block sequence.
    #[error("invalid block sequence: expected {expected}, got {actual}")]
    InvalidBlockSequence {
        /// The expected block number.
        expected: u64,
        /// The actual block number received.
        actual: u64,
    },

    /// Provider error occurred.
    #[error("provider error: {0}")]
    Provider(String),

    /// Task error occurred.
    #[error("task error: {0}")]
    Task(String),

    /// Fatal error occurred.
    #[error(transparent)]
    Fatal(Box<dyn core::error::Error + Send + Sync>),
}

/// Result from a matcher containing matches for a specific map index.
#[derive(Debug, Clone)]
pub struct MatcherResult {
    /// The map index this result is for
    pub map_index: u32,
    /// The potential matches found for this map
    /// None = wildcard (matches all), Some(vec) = specific matches
    pub matches: Vec<u64>,
}

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

/// Information about a completed map
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedMap {
    /// The map index that was completed
    pub map_index: MapIndex,
    /// The first log value index in this map
    pub start_log_value_index: LogValueIndex,
    /// The last log value index in this map (inclusive)
    pub end_log_value_index: LogValueIndex,
    /// The rows that were completed for this map
    pub rows: Vec<(MapRowIndex, FilterMapColumns)>,
}

/// Result of processing a batch of log values
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessBatchResult {
    /// No maps were completed, all values added to current map
    NoMapsCompleted {
        /// Number of log values processed
        values_processed: usize,
    },
    /// One or more maps were completed
    MapsCompleted {
        /// The completed maps with their data
        completed_maps: Vec<CompletedMap>,
        /// Number of log values processed
        values_processed: usize,
    },
}

impl ProcessBatchResult {
    /// Check if any maps were completed
    pub const fn has_completed_maps(&self) -> bool {
        matches!(self, Self::MapsCompleted { .. })
    }

    /// Get completed maps if any
    pub fn completed_maps(self) -> Vec<CompletedMap> {
        match self {
            Self::MapsCompleted { completed_maps, .. } => completed_maps,
            Self::NoMapsCompleted { .. } => Vec::new(),
        }
    }

    /// Get the number of values processed
    pub const fn values_processed(&self) -> usize {
        match self {
            Self::NoMapsCompleted { values_processed } |
            Self::MapsCompleted { values_processed, .. } => *values_processed,
        }
    }
}
