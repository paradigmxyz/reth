use crate::{metrics::EngineApiMetrics, EngineApiError};
use alloy_rpc_types_engine::ExecutionData;
use async_trait::async_trait;
use jsonrpsee_core::RpcResult;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_primitives::PayloadTypes;
use reth_rpc_api::{RethEngineApiServer, RethPayloadStatus};
use std::time::Instant;
use tracing::trace;

/// Standalone implementation of the `reth_` engine API namespace.
///
/// Provides the `reth_newPayload` endpoint that takes `ExecutionData` directly,
/// waits for persistence, execution cache, and sparse trie locks before processing,
/// and returns timing breakdowns with server-measured execution latency.
pub struct RethEngineApi<Payload: PayloadTypes> {
    consensus: ConsensusEngineHandle<Payload>,
    metrics: EngineApiMetrics,
}

impl<Payload: PayloadTypes> std::fmt::Debug for RethEngineApi<Payload> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RethEngineApi").finish_non_exhaustive()
    }
}

impl<Payload: PayloadTypes> RethEngineApi<Payload> {
    /// Creates a new [`RethEngineApi`].
    pub fn new(consensus: ConsensusEngineHandle<Payload>) -> Self {
        Self { consensus, metrics: EngineApiMetrics::default() }
    }
}

#[async_trait]
impl<Payload> RethEngineApiServer for RethEngineApi<Payload>
where
    Payload: PayloadTypes<ExecutionData = ExecutionData>,
{
    async fn reth_new_payload(&self, payload: ExecutionData) -> RpcResult<RethPayloadStatus> {
        trace!(target: "rpc::engine", "Serving reth_newPayload");
        let start = Instant::now();
        let (status, timings) =
            self.consensus.reth_new_payload(payload).await.map_err(EngineApiError::from)?;
        self.metrics.latency.new_payload_v1.record(start.elapsed());
        Ok(RethPayloadStatus {
            status,
            latency_us: timings.latency.as_micros() as u64,
            persistence_wait_us: timings.persistence_wait.map(|d| d.as_micros() as u64),
            execution_cache_wait_us: timings.execution_cache_wait.as_micros() as u64,
            sparse_trie_wait_us: timings.sparse_trie_wait.as_micros() as u64,
        })
    }
}
