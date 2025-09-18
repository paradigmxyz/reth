//! This is an extension trait for any provider that implements the engine API, to wait for a VALID
//! response. This is useful for benchmarking, as it allows us to wait for a payload to be valid
//! before sending additional calls.

use alloy_eips::eip7685::Requests;
use alloy_provider::{ext::EngineApi, network::AnyRpcBlock, Network, Provider};
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadInputV2, ForkchoiceState, ForkchoiceUpdated,
    PayloadAttributes, PayloadStatus,
};
use alloy_transport::TransportResult;
use op_alloy_rpc_types_engine::OpExecutionPayloadV4;
use reth_node_api::EngineApiMessageVersion;
use tracing::error;

/// An extension trait for providers that implement the engine API, to wait for a VALID response.
#[async_trait::async_trait]
pub trait EngineApiValidWaitExt<N>: Send + Sync {
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

pub(crate) fn block_to_new_payload(
    block: AnyRpcBlock,
    is_optimism: bool,
) -> eyre::Result<(EngineApiMessageVersion, serde_json::Value)> {
    let block = block
        .into_inner()
        .map_header(|header| header.map(|h| h.into_header_with_defaults()))
        .try_map_transactions(|tx| {
            // try to convert unknowns into op type so that we can also support optimism
            tx.try_into_either::<op_alloy_consensus::OpTxEnvelope>()
        })?
        .into_consensus();

    // Convert to execution payload
    let (payload, sidecar) = ExecutionPayload::from_block_slow(&block);

    let (version, params) = match payload {
        ExecutionPayload::V3(payload) => {
            let cancun = sidecar.cancun().unwrap();

            if let Some(prague) = sidecar.prague() {
                if is_optimism {
                    (
                        EngineApiMessageVersion::V4,
                        serde_json::to_value((
                            OpExecutionPayloadV4 {
                                payload_inner: payload,
                                withdrawals_root: block.withdrawals_root.unwrap(),
                            },
                            cancun.versioned_hashes.clone(),
                            cancun.parent_beacon_block_root,
                            Requests::default(),
                        ))?,
                    )
                } else {
                    (
                        EngineApiMessageVersion::V4,
                        serde_json::to_value((
                            payload,
                            cancun.versioned_hashes.clone(),
                            cancun.parent_beacon_block_root,
                            prague.requests.requests_hash(),
                        ))?,
                    )
                }
            } else {
                (
                    EngineApiMessageVersion::V3,
                    serde_json::to_value((
                        payload,
                        cancun.versioned_hashes.clone(),
                        cancun.parent_beacon_block_root,
                    ))?,
                )
            }
        }
        ExecutionPayload::V2(payload) => {
            let input = ExecutionPayloadInputV2 {
                execution_payload: payload.payload_inner,
                withdrawals: Some(payload.withdrawals),
            };

            (EngineApiMessageVersion::V2, serde_json::to_value((input,))?)
        }
        ExecutionPayload::V1(payload) => {
            (EngineApiMessageVersion::V1, serde_json::to_value((payload,))?)
        }
    };

    Ok((version, params))
}

/// Calls the correct `engine_newPayload` method depending on the given [`ExecutionPayload`] and its
/// versioned variant. Returns the [`EngineApiMessageVersion`] depending on the payload's version.
///
/// # Panics
/// If the given payload is a V3 payload, but a parent beacon block root is provided as `None`.
pub(crate) async fn call_new_payload<N: Network, P: Provider<N>>(
    provider: P,
    version: EngineApiMessageVersion,
    params: serde_json::Value,
) -> TransportResult<()> {
    let method = version.method_name();

    let mut status: PayloadStatus = provider.client().request(method, &params).await?;

    while !status.is_valid() {
        if status.is_invalid() {
            error!(?status, ?params, "Invalid {method}",);
            panic!("Invalid {method}: {status:?}");
        }
        if status.is_syncing() {
            return Err(alloy_json_rpc::RpcError::UnsupportedFeature(
                "invalid range: no canonical state found for parent of requested block",
            ))
        }
        status = provider.client().request(method, &params).await?;
    }
    Ok(())
}

/// Calls the correct `engine_forkchoiceUpdated` method depending on the given
/// `EngineApiMessageVersion`, using the provided forkchoice state and payload attributes for the
/// actual engine api message call.
pub(crate) async fn call_forkchoice_updated<N, P: EngineApiValidWaitExt<N>>(
    provider: P,
    message_version: EngineApiMessageVersion,
    forkchoice_state: ForkchoiceState,
    payload_attributes: Option<PayloadAttributes>,
) -> TransportResult<ForkchoiceUpdated> {
    match message_version {
        EngineApiMessageVersion::V3 | EngineApiMessageVersion::V4 | EngineApiMessageVersion::V5 => {
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
