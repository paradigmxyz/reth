//! Core types used by the `FilterMaps` implementation.

use alloy_primitives::BlockNumber;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};
use std::vec::Vec;

/// Metadata for tracking the state of log indexing and filter map generation.
///
/// This struct maintains information about which blocks have been indexed and which filter maps
/// have been generated. It is used to track progress and enable resuming of the indexing process.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Copy, Compact)]
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

    /// The index of the first complete filter map that has been generated.
    /// Filter maps before this index may be incomplete or missing.
    pub first_map_index: u32,

    /// The index of the last complete filter map that has been generated.
    /// This tracks how many filter maps have been fully constructed.
    pub last_map_index: u32,

    /// The next log value index that needs to be processed.
    /// Used to resume log indexing from where it left off previously.
    pub next_log_value_index: u64,

    /// The number of maps in the oldest epoch. This is used to be pruning-aware.
    /// When pruning is enabled, we need to know how many maps were created in the oldest epoch
    /// that are still present in the db.
    ///
    /// TODO: need to implement pruning logic in the crate.
    pub oldest_epoch_map_count: u32,
}
/// A row in a filter map stored in the database.
///
/// Each row contains column indices where log values are stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Compact)]
pub struct FilterMapRowEntry {
    pub map_row_index: u64,
    pub columns: Vec<u32>,
}

impl FilterMapRowEntry {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

/// Represents the block boundaries for log value indices.
///
/// Each entry indicates the starting log value index for a specific block number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockBoundary {
    pub block_number: BlockNumber,
    pub log_value_index: u64,
}

/// Errors that can occur when using `FilterMaps`.
#[derive(Debug, thiserror::Error)]
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
}

/// Result type for `FilterMaps` operations.
pub type FilterResult<T> = Result<T, FilterError>;

impl From<reth_errors::ProviderError> for FilterError {
    fn from(err: reth_errors::ProviderError) -> Self {
        Self::Provider(err.to_string())
    }
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
