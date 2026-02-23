//! Reth-specific engine API extensions.

use alloy_rpc_types_engine::PayloadStatus;
use serde::{Deserialize, Serialize};

/// Reth-specific payload status that includes server-measured execution latency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RethPayloadStatus {
    /// The standard payload status.
    #[serde(flatten)]
    pub status: PayloadStatus,
    /// Server-side execution latency in microseconds.
    pub latency_us: u64,
    /// Time spent waiting for persistence to complete, in microseconds.
    /// `None` when no persistence was in-flight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_wait_us: Option<u64>,
    /// Time spent waiting for the execution cache lock, in microseconds.
    pub execution_cache_wait_us: u64,
    /// Time spent waiting for the sparse trie lock, in microseconds.
    pub sparse_trie_wait_us: u64,
}
