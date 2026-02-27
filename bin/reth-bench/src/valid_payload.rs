//! This is an extension trait for any provider that implements the engine API, to wait for a VALID
//! response. This is useful for benchmarking, as it allows us to wait for a payload to be valid
//! before sending additional calls.

use alloy_eips::eip7685::Requests;
use alloy_primitives::{Bytes, B256};
use alloy_provider::{ext::EngineApi, network::AnyRpcBlock, Network, Provider};
use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayload, ExecutionPayloadInputV2, ExecutionPayloadSidecar,
    ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use alloy_transport::TransportResult;
use op_alloy_rpc_types_engine::OpExecutionPayloadV4;
use reth_node_api::EngineApiMessageVersion;
use reth_rpc_api::RethNewPayloadInput;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, error};

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
        debug!(
            target: "reth-bench",
            method = "engine_forkchoiceUpdatedV1",
            ?fork_choice_state,
            ?payload_attributes,
            "Sending forkchoiceUpdated"
        );

        let mut status =
            self.fork_choice_updated_v1(fork_choice_state, payload_attributes.clone()).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    target: "reth-bench",
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
        debug!(
            target: "reth-bench",
            method = "engine_forkchoiceUpdatedV2",
            ?fork_choice_state,
            ?payload_attributes,
            "Sending forkchoiceUpdated"
        );

        let mut status =
            self.fork_choice_updated_v2(fork_choice_state, payload_attributes.clone()).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    target: "reth-bench",
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
        debug!(
            target: "reth-bench",
            method = "engine_forkchoiceUpdatedV3",
            ?fork_choice_state,
            ?payload_attributes,
            "Sending forkchoiceUpdated"
        );

        let mut status =
            self.fork_choice_updated_v3(fork_choice_state, payload_attributes.clone()).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    target: "reth-bench",
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

/// Converts an RPC block into versioned engine API params and an [`ExecutionData`].
///
/// Returns `(version, versioned_params, execution_data)`.
pub(crate) fn block_to_new_payload(
    block: AnyRpcBlock,
    is_optimism: bool,
    rlp: Option<Bytes>,
    reth_new_payload: bool,
) -> eyre::Result<(Option<EngineApiMessageVersion>, serde_json::Value)> {
    if let Some(rlp) = rlp {
        return Ok((
            None,
            serde_json::to_value((RethNewPayloadInput::<ExecutionData>::BlockRlp(rlp),))?,
        ));
    }
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
    let (version, params, execution_data) =
        payload_to_new_payload(payload, sidecar, is_optimism, block.withdrawals_root, None)?;

    if reth_new_payload {
        Ok((None, serde_json::to_value((RethNewPayloadInput::ExecutionData(execution_data),))?))
    } else {
        Ok((Some(version), params))
    }
}

/// Converts an execution payload and sidecar into versioned engine API params and an
/// [`ExecutionData`].
///
/// Returns `(version, versioned_params, execution_data)`.
pub(crate) fn payload_to_new_payload(
    payload: ExecutionPayload,
    sidecar: ExecutionPayloadSidecar,
    is_optimism: bool,
    withdrawals_root: Option<B256>,
    target_version: Option<EngineApiMessageVersion>,
) -> eyre::Result<(EngineApiMessageVersion, serde_json::Value, ExecutionData)> {
    let execution_data = ExecutionData { payload: payload.clone(), sidecar: sidecar.clone() };

    let (version, params) = match payload {
        ExecutionPayload::V3(payload) => {
            let cancun = sidecar.cancun().unwrap();

            if let Some(prague) = sidecar.prague() {
                // Use target version if provided (for Osaka), otherwise default to V4
                let version = target_version.unwrap_or(EngineApiMessageVersion::V4);

                if is_optimism {
                    let withdrawals_root = withdrawals_root.ok_or_else(|| {
                        eyre::eyre!("Missing withdrawals root for Optimism payload")
                    })?;
                    (
                        version,
                        serde_json::to_value((
                            OpExecutionPayloadV4 { payload_inner: payload, withdrawals_root },
                            cancun.versioned_hashes.clone(),
                            cancun.parent_beacon_block_root,
                            Requests::default(),
                        ))?,
                    )
                } else {
                    // Extract actual Requests from RequestsOrHash
                    let requests = prague.requests.requests_hash();
                    (
                        version,
                        serde_json::to_value((
                            payload,
                            cancun.versioned_hashes.clone(),
                            cancun.parent_beacon_block_root,
                            requests,
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

    Ok((version, params, execution_data))
}

/// Calls the correct `engine_newPayload` method depending on the given [`ExecutionPayload`] and its
/// versioned variant. Returns the [`EngineApiMessageVersion`] depending on the payload's version.
///
/// # Panics
/// If the given payload is a V3 payload, but a parent beacon block root is provided as `None`.
#[allow(dead_code)]
pub(crate) async fn call_new_payload<N: Network, P: Provider<N>>(
    provider: P,
    version: Option<EngineApiMessageVersion>,
    params: serde_json::Value,
) -> eyre::Result<Option<NewPayloadTimingBreakdown>> {
    call_new_payload_with_reth(provider, version, params).await
}

/// Response from `reth_newPayload` endpoint, which includes server-measured latency.
#[derive(Debug, Deserialize)]
struct RethPayloadStatus {
    latency_us: u64,
    #[serde(default)]
    persistence_wait_us: Option<u64>,
    #[serde(default)]
    execution_cache_wait_us: u64,
    #[serde(default)]
    sparse_trie_wait_us: u64,
}

/// Server-side timing breakdown from `reth_newPayload` endpoint.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NewPayloadTimingBreakdown {
    /// Server-side execution latency.
    pub(crate) latency: Duration,
    /// Time spent waiting for persistence. `None` when no persistence was in-flight.
    pub(crate) persistence_wait: Option<Duration>,
    /// Time spent waiting for execution cache lock.
    pub(crate) execution_cache_wait: Duration,
    /// Time spent waiting for sparse trie lock.
    pub(crate) sparse_trie_wait: Duration,
}

/// Calls either `engine_newPayload*` or `reth_newPayload` depending on whether
/// `version` is provided.
///
/// When `version` is `None`, uses `reth_newPayload` endpoint with provided params.
///
/// Returns the server-reported timing breakdown when using the reth namespace, or `None` for
/// the standard engine namespace.
pub(crate) async fn call_new_payload_with_reth<N: Network, P: Provider<N>>(
    provider: P,
    version: Option<EngineApiMessageVersion>,
    params: serde_json::Value,
) -> eyre::Result<Option<NewPayloadTimingBreakdown>> {
    let method = version.map(|v| v.method_name()).unwrap_or("reth_newPayload");

    debug!(target: "reth-bench", method, "Sending newPayload");

    let resp = loop {
        let resp: serde_json::Value = provider.client().request(method, &params).await?;
        let status = PayloadStatus::deserialize(&resp)?;

        if status.is_valid() {
            break resp;
        }
        if status.is_invalid() {
            return Err(eyre::eyre!("Invalid {method}: {status:?}"));
        }
        if status.is_syncing() {
            return Err(eyre::eyre!(
                "invalid range: no canonical state found for parent of requested block"
            ));
        }
    };

    if version.is_some() {
        return Ok(None);
    }

    let resp: RethPayloadStatus = serde_json::from_value(resp)?;

    Ok(Some(NewPayloadTimingBreakdown {
        latency: Duration::from_micros(resp.latency_us),
        persistence_wait: resp.persistence_wait_us.map(Duration::from_micros),
        execution_cache_wait: Duration::from_micros(resp.execution_cache_wait_us),
        sparse_trie_wait: Duration::from_micros(resp.sparse_trie_wait_us),
    }))
}

/// Calls the correct `engine_forkchoiceUpdated` method depending on the given
/// `EngineApiMessageVersion`, using the provided forkchoice state and payload attributes for the
/// actual engine api message call.
///
/// Note: For Prague (V4), we still use forkchoiceUpdatedV3 as there is no V4.
pub(crate) async fn call_forkchoice_updated<N, P: EngineApiValidWaitExt<N>>(
    provider: P,
    message_version: EngineApiMessageVersion,
    forkchoice_state: ForkchoiceState,
    payload_attributes: Option<PayloadAttributes>,
) -> TransportResult<ForkchoiceUpdated> {
    // FCU V3 is used for both Cancun and Prague (there is no FCU V4)
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

/// Calls either `reth_forkchoiceUpdated` or the standard `engine_forkchoiceUpdated*` depending
/// on `use_reth`.
///
/// When `use_reth` is true, uses the `reth_forkchoiceUpdated` endpoint which sends a regular FCU
/// with no payload attributes.
pub(crate) async fn call_forkchoice_updated_with_reth<
    N: Network,
    P: Provider<N> + EngineApiValidWaitExt<N>,
>(
    provider: P,
    message_version: Option<EngineApiMessageVersion>,
    forkchoice_state: ForkchoiceState,
) -> TransportResult<ForkchoiceUpdated> {
    if let Some(message_version) = message_version {
        call_forkchoice_updated(provider, message_version, forkchoice_state, None).await
    } else {
        let method = "reth_forkchoiceUpdated";
        let reth_params = serde_json::to_value((forkchoice_state,))
            .expect("ForkchoiceState serialization cannot fail");

        debug!(target: "reth-bench", method, "Sending forkchoiceUpdated");

        loop {
            let resp: ForkchoiceUpdated = provider.client().request(method, &reth_params).await?;

            if resp.is_valid() {
                break Ok(resp)
            }

            if resp.is_invalid() {
                error!(target: "reth-bench", ?resp, "Invalid {method}");
                return Err(alloy_json_rpc::RpcError::LocalUsageError(Box::new(
                    std::io::Error::other(format!("Invalid {method}: {resp:?}")),
                )))
            }
            if resp.is_syncing() {
                return Err(alloy_json_rpc::RpcError::UnsupportedFeature(
                    "invalid range: no canonical state found for parent of requested block",
                ))
            }
        }
    }
}
