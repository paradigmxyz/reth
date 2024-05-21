//! This is an extension trait for any provider that implements the engine API, to wait for a VALID
//! response. This is useful for benchmarking, as it allows us to wait for a payload to be valid
//! before sending additional calls.

use alloy_provider::{ext::EngineApi, Network};
use alloy_rpc_types_engine::{ExecutionPayloadInputV2, PayloadStatus, PayloadStatusEnum};
use alloy_transport::{Transport, TransportResult};
use reth_primitives::B256;
use reth_rpc_types::{ExecutionPayloadV1, ExecutionPayloadV3};

/// An extension trait for providers that implement the engine API, to wait for a VALID response.
#[async_trait::async_trait]
pub trait EngineApiValidWaitExt<N, T>: Send + Sync {
    /// Calls `engine_newPayloadV1` with the given [ExecutionPayloadV1], and waits until the
    /// response is VALID.
    async fn new_payload_v1_wait(
        &self,
        payload: ExecutionPayloadV1,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_newPayloadV2` with the given [ExecutionPayloadV2], and waits until the
    /// response is VALID.
    async fn new_payload_v2_wait(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_newPayloadV3` with the given [ExecutionPayloadV3], parent beacon block root,
    /// and versioned hashes, and waits until the response is VALID.
    async fn new_payload_v3_wait(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> TransportResult<PayloadStatus>;

    // TODO: add forkchoiceUpdated
}

#[async_trait::async_trait]
impl<N, T, P> EngineApiValidWaitExt<N, T> for P
where
    N: Network,
    T: Transport + Clone,
    P: EngineApi<T, N>,
{
    async fn new_payload_v1_wait(
        &self,
        payload: ExecutionPayloadV1,
    ) -> TransportResult<PayloadStatus> {
        // TODO: remove clones somehow?
        let mut status = self.new_payload_v1(payload.clone()).await?;
        while status.status != PayloadStatusEnum::Valid {
            status = self.new_payload_v1(payload.clone()).await?;
        }
        Ok(status)
    }

    async fn new_payload_v2_wait(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> TransportResult<PayloadStatus> {
        // TODO: remove clones somehow?
        let mut status = self.new_payload_v2(payload.clone()).await?;
        while status.status != PayloadStatusEnum::Valid {
            status = self.new_payload_v2(payload.clone()).await?;
        }
        Ok(status)
    }

    async fn new_payload_v3_wait(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> TransportResult<PayloadStatus> {
        // TODO: remove clones somehow?
        let mut status = self
            .new_payload_v3(payload.clone(), versioned_hashes.clone(), parent_beacon_block_root)
            .await?;
        while status.status != PayloadStatusEnum::Valid {
            status = self
                .new_payload_v3(payload.clone(), versioned_hashes.clone(), parent_beacon_block_root)
                .await?;
        }
        Ok(status)
    }
}
