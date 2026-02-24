//! Reth-specific engine API extensions.

use alloy_primitives::Bytes;
use alloy_rpc_types_engine::PayloadStatus;
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

/// Input for `reth_newPayload` that accepts either `ExecutionData` directly or an RLP-encoded
/// block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RethNewPayloadInput<ExecutionData> {
    /// Standard execution data (payload + sidecar).
    ExecutionData(ExecutionData),
    /// An RLP-encoded block.
    BlockRlp(Bytes),
}

/// Reth-specific engine API extensions.
///
/// This trait provides a `reth_newPayload` endpoint that accepts either `ExecutionData` directly
/// (payload + sidecar) or an RLP-encoded block, waiting for persistence and cache locks before
/// processing.
///
/// Responses include timing breakdowns with server-measured execution latency.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "reth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "reth"))]
pub trait RethEngineApi<ExecutionData> {
    /// Reth-specific newPayload that accepts either `ExecutionData` directly or an RLP-encoded
    /// block.
    ///
    /// Waits for persistence, execution cache, and sparse trie locks before processing.
    #[method(name = "newPayload")]
    async fn reth_new_payload(
        &self,
        payload: RethNewPayloadInput<ExecutionData>,
    ) -> RpcResult<RethPayloadStatus>;
}
