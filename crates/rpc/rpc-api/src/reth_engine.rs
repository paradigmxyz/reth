//! Reth-specific engine API extensions.

use alloy_rpc_types_engine::{ExecutionData, PayloadStatus};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
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

/// Reth-specific engine API extensions.
///
/// This trait provides a `reth_newPayload` endpoint that takes `ExecutionData` directly
/// (payload + sidecar), waiting for persistence and cache locks before processing.
///
/// Responses include timing breakdowns with server-measured execution latency.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "reth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "reth"))]
pub trait RethEngineApi {
    /// Reth-specific newPayload that takes `ExecutionData` directly.
    ///
    /// Waits for persistence, execution cache, and sparse trie locks before processing.
    #[method(name = "newPayload")]
    async fn reth_new_payload(&self, payload: ExecutionData) -> RpcResult<RethPayloadStatus>;
}
