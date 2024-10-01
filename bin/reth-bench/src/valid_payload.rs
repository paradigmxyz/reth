//! This is an extension trait for any provider that implements the engine API, to wait for a VALID
//! response. This is useful for benchmarking, as it allows us to wait for a payload to be valid
//! before sending additional calls.

use alloy_primitives::B256;
use alloy_provider::{ext::EngineApi, Network};
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadInputV2, ExecutionPayloadV1, ExecutionPayloadV3,
    ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use alloy_transport::{Transport, TransportResult};
use reth_node_api::EngineApiMessageVersion;
use tracing::error;

/// An extension trait for providers that implement the engine API, to wait for a VALID response.
#[async_trait::async_trait]
pub trait EngineApiValidWaitExt<N, T>: Send + Sync {
    /// Calls `engine_newPayloadV1` with the given [ExecutionPayloadV1], and waits until the
    /// response is VALID.
    async fn new_payload_v1_wait(
        &self,
        payload: ExecutionPayloadV1,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_newPayloadV2` with the given [ExecutionPayloadInputV2], and waits until the
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

    /// Calls `engine_forkChoiceUpdatedV1` with the given [ForkchoiceState] and optional
    /// [PayloadAttributes], and waits until the response is VALID.
    async fn fork_choice_updated_v1_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated>;

    /// Calls `engine_forkChoiceUpdatedV2` with the given [ForkchoiceState] and optional
    /// [PayloadAttributes], and waits until the response is VALID.
    async fn fork_choice_updated_v2_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated>;

    /// Calls `engine_forkChoiceUpdatedV3` with the given [ForkchoiceState] and optional
    /// [PayloadAttributes], and waits until the response is VALID.
    async fn fork_choice_updated_v3_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated>;
}

#[async_trait::async_trait]
impl<T, N, P> EngineApiValidWaitExt<N, T> for P
where
    N: Network,
    T: Transport + Clone,
    P: EngineApi<N, T>,
{
    async fn new_payload_v1_wait(
        &self,
        payload: ExecutionPayloadV1,
    ) -> TransportResult<PayloadStatus> {
        let mut status = self.new_payload_v1(payload.clone()).await?;
        while !status.is_valid() {
            if status.is_invalid() {
                error!(?status, ?payload, "Invalid newPayloadV1",);
                panic!("Invalid newPayloadV1: {status:?}");
            }
            status = self.new_payload_v1(payload.clone()).await?;
        }
        Ok(status)
    }

    async fn new_payload_v2_wait(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> TransportResult<PayloadStatus> {
        let mut status = self.new_payload_v2(payload.clone()).await?;
        while !status.is_valid() {
            if status.is_invalid() {
                error!(?status, ?payload, "Invalid newPayloadV2",);
                panic!("Invalid newPayloadV2: {status:?}");
            }
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
        let mut status = self
            .new_payload_v3(payload.clone(), versioned_hashes.clone(), parent_beacon_block_root)
            .await?;
        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    ?status,
                    ?payload,
                    ?versioned_hashes,
                    ?parent_beacon_block_root,
                    "Invalid newPayloadV3",
                );
                panic!("Invalid newPayloadV3: {status:?}");
            }
            status = self
                .new_payload_v3(payload.clone(), versioned_hashes.clone(), parent_beacon_block_root)
                .await?;
        }
        Ok(status)
    }

    async fn fork_choice_updated_v1_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated> {
        let mut status =
            self.fork_choice_updated_v1(fork_choice_state, payload_attributes.clone()).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    ?status,
                    ?fork_choice_state,
                    ?payload_attributes,
                    "Invalid forkchoiceUpdatedV1 message",
                );
                panic!("Invalid forkchoiceUpdatedV1: {status:?}");
            }
            status =
                self.fork_choice_updated_v1(fork_choice_state, payload_attributes.clone()).await?;
        }

        Ok(status)
    }

    async fn fork_choice_updated_v2_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated> {
        let mut status =
            self.fork_choice_updated_v2(fork_choice_state, payload_attributes.clone()).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    ?status,
                    ?fork_choice_state,
                    ?payload_attributes,
                    "Invalid forkchoiceUpdatedV2 message",
                );
                panic!("Invalid forkchoiceUpdatedV2: {status:?}");
            }
            status =
                self.fork_choice_updated_v2(fork_choice_state, payload_attributes.clone()).await?;
        }

        Ok(status)
    }

    async fn fork_choice_updated_v3_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated> {
        let mut status =
            self.fork_choice_updated_v3(fork_choice_state, payload_attributes.clone()).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    ?status,
                    ?fork_choice_state,
                    ?payload_attributes,
                    "Invalid forkchoiceUpdatedV3 message",
                );
                panic!("Invalid forkchoiceUpdatedV3: {status:?}");
            }
            status =
                self.fork_choice_updated_v3(fork_choice_state, payload_attributes.clone()).await?;
        }

        Ok(status)
    }
}

/// Calls the correct `engine_newPayload` method depending on the given [`ExecutionPayload`] and its
/// versioned variant. Returns the [`EngineApiMessageVersion`] depending on the payload's version.
///
/// # Panics
/// If the given payload is a V3 payload, but a parent beacon block root is provided as `None`.
pub(crate) async fn call_new_payload<N, T, P: EngineApiValidWaitExt<N, T>>(
    provider: P,
    payload: ExecutionPayload,
    parent_beacon_block_root: Option<B256>,
    versioned_hashes: Vec<B256>,
) -> TransportResult<EngineApiMessageVersion> {
    match payload {
        ExecutionPayload::V4(_payload) => {
            todo!("V4 payloads not supported yet");
            // auth_provider
            //     .new_payload_v4_wait(payload, versioned_hashes, parent_beacon_block_root, ...)
            //     .await?;
            //
            // Ok(EngineApiMessageVersion::V4)
        }
        ExecutionPayload::V3(payload) => {
            // We expect the caller
            let parent_beacon_block_root = parent_beacon_block_root
                .expect("parent_beacon_block_root is required for V3 payloads");
            provider
                .new_payload_v3_wait(payload, versioned_hashes, parent_beacon_block_root)
                .await?;

            Ok(EngineApiMessageVersion::V3)
        }
        ExecutionPayload::V2(payload) => {
            let input = ExecutionPayloadInputV2 {
                execution_payload: payload.payload_inner,
                withdrawals: Some(payload.withdrawals),
            };

            provider.new_payload_v2_wait(input).await?;

            Ok(EngineApiMessageVersion::V2)
        }
        ExecutionPayload::V1(payload) => {
            provider.new_payload_v1_wait(payload).await?;

            Ok(EngineApiMessageVersion::V1)
        }
    }
}

/// Calls the correct `engine_forkchoiceUpdated` method depending on the given
/// `EngineApiMessageVersion`, using the provided forkchoice state and payload attributes for the
/// actual engine api message call.
pub(crate) async fn call_forkchoice_updated<N, T, P: EngineApiValidWaitExt<N, T>>(
    provider: P,
    message_version: EngineApiMessageVersion,
    forkchoice_state: ForkchoiceState,
    payload_attributes: Option<PayloadAttributes>,
) -> TransportResult<ForkchoiceUpdated> {
    match message_version {
        EngineApiMessageVersion::V4 => todo!("V4 payloads not supported yet"),
        EngineApiMessageVersion::V3 => {
            provider.fork_choice_updated_v3_wait(forkchoice_state, payload_attributes).await
        }
        EngineApiMessageVersion::V2 => {
            provider.fork_choice_updated_v2_wait(forkchoice_state, payload_attributes).await
        }
        EngineApiMessageVersion::V1 => {
            provider.fork_choice_updated_v1_wait(forkchoice_state, payload_attributes).await
        }
    }
}
