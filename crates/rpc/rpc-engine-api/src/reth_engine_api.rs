use crate::EngineApiError;
use alloy_rlp::Decodable;
use async_trait::async_trait;
use jsonrpsee_core::RpcResult;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_primitives::PayloadTypes;
use reth_primitives_traits::SealedBlock;
use reth_rpc_api::{RethEngineApiServer, RethNewPayloadInput, RethPayloadStatus};
use tracing::trace;

/// Standalone implementation of the `reth_` engine API namespace.
///
/// Provides the `reth_newPayload` endpoint that accepts either `ExecutionData` directly or an
/// RLP-encoded block, waits for persistence, execution cache, and sparse trie locks before
/// processing, and returns timing breakdowns with server-measured execution latency.
#[derive(Debug)]
pub struct RethEngineApi<Payload: PayloadTypes> {
    beacon_engine_handle: ConsensusEngineHandle<Payload>,
}

impl<Payload: PayloadTypes> RethEngineApi<Payload> {
    /// Creates a new [`RethEngineApi`].
    pub const fn new(beacon_engine_handle: ConsensusEngineHandle<Payload>) -> Self {
        Self { beacon_engine_handle }
    }
}

#[async_trait]
impl<Payload: PayloadTypes> RethEngineApiServer<Payload::ExecutionData> for RethEngineApi<Payload> {
    async fn reth_new_payload(
        &self,
        input: RethNewPayloadInput<Payload::ExecutionData>,
    ) -> RpcResult<RethPayloadStatus> {
        trace!(target: "rpc::engine", "Serving reth_newPayload");

        let payload = match input {
            RethNewPayloadInput::ExecutionData(data) => data,
            RethNewPayloadInput::BlockRlp(rlp) => {
                let block = Decodable::decode(&mut rlp.as_ref())
                    .map_err(|err| EngineApiError::Internal(Box::new(err)))?;
                Payload::block_to_payload(SealedBlock::new_unhashed(block))
            }
        };

        let (status, timings) = self
            .beacon_engine_handle
            .reth_new_payload(payload)
            .await
            .map_err(EngineApiError::from)?;
        Ok(RethPayloadStatus {
            status,
            latency_us: timings.latency.as_micros() as u64,
            persistence_wait_us: timings.persistence_wait.map(|d| d.as_micros() as u64),
            execution_cache_wait_us: timings.execution_cache_wait.as_micros() as u64,
            sparse_trie_wait_us: timings.sparse_trie_wait.as_micros() as u64,
        })
    }
}
