//! This is an extension trait for any provider that implements the engine API, to wait for a VALID
//! response. This is useful for benchmarking, as it allows us to wait for a payload to be valid
//! before sending additional calls.

use alloy_eips::eip7685::RequestsOrHash;
use alloy_primitives::B256;
use alloy_provider::{ext::EngineApi, Network, Provider};
use alloy_rpc_types_engine::{
    ExecutionPayloadInputV2, ExecutionPayloadV1, ExecutionPayloadV3, ForkchoiceState,
    ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use alloy_transport::TransportResult;
use tracing::error;

use crate::bench::EngineRequests;

/// An extension trait for providers that implement the engine API, to wait for a VALID response.
#[async_trait::async_trait]
pub trait EngineApiValidWaitExt<N>: Send + Sync {
    /// Calls `engine_newPayloadV1` with the given [`ExecutionPayloadV1`], and waits until the
    /// response is VALID.
    async fn new_payload_v1_wait(
        &self,
        payload: ExecutionPayloadV1,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_newPayloadV2` with the given [`ExecutionPayloadInputV2`], and waits until the
    /// response is VALID.
    async fn new_payload_v2_wait(
        &self,
        payload: ExecutionPayloadInputV2,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_newPayloadV3` with the given [`ExecutionPayloadV3`], parent beacon block root,
    /// and versioned hashes, and waits until the response is VALID.
    async fn new_payload_v3_wait(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_newPayloadV4` with the given [`ExecutionPayloadV3`], parent beacon block root,
    /// versioned hashes, and requests hash, and waits until the response is VALID.
    async fn new_payload_v4_wait(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        requests_hash: B256,
    ) -> TransportResult<PayloadStatus>;

    /// Calls `engine_forkChoiceUpdatedV1` with the given [`ForkchoiceState`] and optional
    /// [`PayloadAttributes`], and waits until the response is VALID.
    async fn fork_choice_updated_v1_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated>;

    /// Calls `engine_forkChoiceUpdatedV2` with the given [`ForkchoiceState`] and optional
    /// [`PayloadAttributes`], and waits until the response is VALID.
    async fn fork_choice_updated_v2_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated>;

    /// Calls `engine_forkChoiceUpdatedV3` with the given [`ForkchoiceState`] and optional
    /// [`PayloadAttributes`], and waits until the response is VALID.
    async fn fork_choice_updated_v3_wait(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated>;
}

#[async_trait::async_trait]
impl<N, P> EngineApiValidWaitExt<N> for P
where
    N: Network,
    P: Provider<N> + EngineApi<N>,
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
            if status.is_syncing() {
                return Err(alloy_json_rpc::RpcError::UnsupportedFeature(
                    "invalid range: no canonical state found for parent of requested block",
                ))
            }
            status = self
                .new_payload_v3(payload.clone(), versioned_hashes.clone(), parent_beacon_block_root)
                .await?;
        }
        Ok(status)
    }

    async fn new_payload_v4_wait(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        requests_hash: B256,
    ) -> TransportResult<PayloadStatus> {
        // We cannot use `self.new_payload_v4` because it does not support sending
        // `RequestsOrHash::Hash`

        let mut status: PayloadStatus = self
            .client()
            .request(
                "engine_newPayloadV4",
                (
                    payload.clone(),
                    versioned_hashes.clone(),
                    parent_beacon_block_root,
                    RequestsOrHash::Hash(requests_hash),
                ),
            )
            .await?;
        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    ?status,
                    ?payload,
                    ?versioned_hashes,
                    ?parent_beacon_block_root,
                    "Invalid newPayloadV4",
                );
                panic!("Invalid newPayloadV4: {status:?}");
            }
            if status.is_syncing() {
                return Err(alloy_json_rpc::RpcError::UnsupportedFeature(
                    "invalid range: no canonical state found for parent of requested block",
                ))
            }
            status = self
                .client()
                .request(
                    "engine_newPayloadV4",
                    (
                        payload.clone(),
                        versioned_hashes.clone(),
                        parent_beacon_block_root,
                        RequestsOrHash::Hash(requests_hash),
                    ),
                )
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
            if status.is_syncing() {
                return Err(alloy_json_rpc::RpcError::UnsupportedFeature(
                    "invalid range: no canonical state found for parent of requested block",
                ))
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
            if status.is_syncing() {
                return Err(alloy_json_rpc::RpcError::UnsupportedFeature(
                    "invalid range: no canonical state found for parent of requested block",
                ))
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
pub(crate) async fn call_new_payload<N: Network, P: Provider<N>>(
    provider: P,
    request: &EngineRequests,
) -> TransportResult<()> {
    let EngineRequests { method, params, .. } = request;
    let mut status: PayloadStatus = provider.client().request(method.clone(), params).await?;
    while !status.is_valid() {
        if status.is_invalid() {
            error!(?status, ?params, "Invalid {method}");
            panic!("Invalid {method}: {status:?}");
        }
        status = provider.client().request(method.clone(), params).await?;
    }
    Ok(())
}

/// Calls the correct `engine_forkchoiceUpdated` method depending on the given
/// `EngineApiMessageVersion`, using the provided forkchoice state and payload attributes for the
/// actual engine api message call.
pub(crate) async fn call_forkchoice_updated<N: Network, P: Provider<N>>(
    provider: P,
    request: &EngineRequests,
    forkchoice_state: ForkchoiceState,
) -> TransportResult<()> {
    let EngineRequests { fcu_method, .. } = request;

    let params = (forkchoice_state, None::<PayloadAttributes>);

    let mut status: ForkchoiceUpdated =
        provider.client().request(fcu_method.clone(), &params).await?;

    while !status.is_valid() {
        if status.is_invalid() {
            error!(?status, ?request, "Invalid {fcu_method} message",);
            panic!("Invalid {fcu_method}: {status:?}");
        }
        status = provider.client().request(fcu_method.clone(), &params).await?;
    }

    Ok(())
}
