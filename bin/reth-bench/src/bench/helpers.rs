//! Common helpers for reth-bench commands.

use crate::valid_payload::call_forkchoice_updated;
use alloy_consensus::Header;
use alloy_eips::eip4844::kzg_to_versioned_hash;
use alloy_primitives::{Address, B256};
use alloy_provider::{ext::EngineApi, network::AnyNetwork, RootProvider};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar, ForkchoiceState,
    PayloadAttributes, PayloadId, PraguePayloadFields,
};
use eyre::OptionExt;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_node_api::EngineApiMessageVersion;
use tracing::debug;

/// Prepared payload request data for triggering block building.
pub(crate) struct PayloadRequest {
    /// The payload attributes for the new block.
    pub(crate) attributes: PayloadAttributes,
    /// The forkchoice state pointing to the parent block.
    pub(crate) forkchoice_state: ForkchoiceState,
    /// The engine API version for FCU calls.
    pub(crate) fcu_version: EngineApiMessageVersion,
    /// The getPayload version to use (1-5).
    pub(crate) get_payload_version: u8,
    /// The newPayload version to use.
    pub(crate) new_payload_version: EngineApiMessageVersion,
}

/// Prepare payload attributes and forkchoice state for a new block.
pub(crate) fn prepare_payload_request(
    chain_spec: &ChainSpec,
    timestamp: u64,
    parent_hash: B256,
) -> PayloadRequest {
    let shanghai_active = chain_spec.is_shanghai_active_at_timestamp(timestamp);
    let cancun_active = chain_spec.is_cancun_active_at_timestamp(timestamp);
    let prague_active = chain_spec.is_prague_active_at_timestamp(timestamp);
    let osaka_active = chain_spec.is_osaka_active_at_timestamp(timestamp);

    // FCU version: V3 for Cancun+Prague+Osaka, V2 for Shanghai, V1 otherwise
    let fcu_version = if cancun_active {
        EngineApiMessageVersion::V3
    } else if shanghai_active {
        EngineApiMessageVersion::V2
    } else {
        EngineApiMessageVersion::V1
    };

    // getPayload version: 5 for Osaka, 4 for Prague, 3 for Cancun, 2 for Shanghai, 1 otherwise
    // newPayload version: 4 for Prague+Osaka (no V5), 3 for Cancun, 2 for Shanghai, 1 otherwise
    let (get_payload_version, new_payload_version) = if osaka_active {
        (5, EngineApiMessageVersion::V4) // Osaka uses getPayloadV5 but newPayloadV4
    } else if prague_active {
        (4, EngineApiMessageVersion::V4)
    } else if cancun_active {
        (3, EngineApiMessageVersion::V3)
    } else if shanghai_active {
        (2, EngineApiMessageVersion::V2)
    } else {
        (1, EngineApiMessageVersion::V1)
    };

    PayloadRequest {
        attributes: PayloadAttributes {
            timestamp,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: shanghai_active.then(Vec::new),
            parent_beacon_block_root: cancun_active.then_some(B256::ZERO),
        },
        forkchoice_state: ForkchoiceState {
            head_block_hash: parent_hash,
            safe_block_hash: parent_hash,
            finalized_block_hash: parent_hash,
        },
        fcu_version,
        get_payload_version,
        new_payload_version,
    }
}

/// Trigger payload building via FCU and retrieve the built payload.
///
/// This sends a forkchoiceUpdated with payload attributes to start building,
/// then calls getPayload to retrieve the result.
pub(crate) async fn build_payload(
    provider: &RootProvider<AnyNetwork>,
    request: PayloadRequest,
) -> eyre::Result<(ExecutionPayload, ExecutionPayloadSidecar)> {
    let fcu_result = call_forkchoice_updated(
        provider,
        request.fcu_version,
        request.forkchoice_state,
        Some(request.attributes.clone()),
    )
    .await?;

    let payload_id =
        fcu_result.payload_id.ok_or_eyre("Payload builder did not return a payload id")?;

    get_payload_with_sidecar(
        provider,
        request.get_payload_version,
        payload_id,
        request.attributes.parent_beacon_block_root,
    )
    .await
}

/// Convert an RPC block to a consensus header and block hash.
pub(crate) fn rpc_block_to_header(block: alloy_provider::network::AnyRpcBlock) -> (Header, B256) {
    let block_hash = block.header.hash;
    let header = block.header.inner.clone().into_header_with_defaults();
    (header, block_hash)
}

/// Compute versioned hashes from KZG commitments.
fn versioned_hashes_from_commitments(
    commitments: &[alloy_primitives::FixedBytes<48>],
) -> Vec<B256> {
    commitments.iter().map(|c| kzg_to_versioned_hash(c.as_ref())).collect()
}

/// Fetch an execution payload using the appropriate engine API version.
pub(crate) async fn get_payload_with_sidecar(
    provider: &RootProvider<AnyNetwork>,
    version: u8,
    payload_id: PayloadId,
    parent_beacon_block_root: Option<B256>,
) -> eyre::Result<(ExecutionPayload, ExecutionPayloadSidecar)> {
    let method = match version {
        1 => "engine_getPayloadV1",
        2 => "engine_getPayloadV2",
        3 => "engine_getPayloadV3",
        4 => "engine_getPayloadV4",
        5 => "engine_getPayloadV5",
        _ => return Err(eyre::eyre!("Unsupported getPayload version: {}", version)),
    };

    debug!(
        target: "reth-bench",
        method,
        ?payload_id,
        "Sending getPayload"
    );

    match version {
        1 => {
            let payload = provider.get_payload_v1(payload_id).await?;
            Ok((ExecutionPayload::V1(payload), ExecutionPayloadSidecar::none()))
        }
        2 => {
            let envelope = provider.get_payload_v2(payload_id).await?;
            let payload = match envelope.execution_payload {
                alloy_rpc_types_engine::ExecutionPayloadFieldV2::V1(p) => ExecutionPayload::V1(p),
                alloy_rpc_types_engine::ExecutionPayloadFieldV2::V2(p) => ExecutionPayload::V2(p),
            };
            Ok((payload, ExecutionPayloadSidecar::none()))
        }
        3 => {
            let envelope = provider.get_payload_v3(payload_id).await?;
            let versioned_hashes =
                versioned_hashes_from_commitments(&envelope.blobs_bundle.commitments);
            let cancun_fields = CancunPayloadFields {
                parent_beacon_block_root: parent_beacon_block_root
                    .ok_or_eyre("parent_beacon_block_root required for V3")?,
                versioned_hashes,
            };
            Ok((
                ExecutionPayload::V3(envelope.execution_payload),
                ExecutionPayloadSidecar::v3(cancun_fields),
            ))
        }
        4 => {
            let envelope = provider.get_payload_v4(payload_id).await?;
            let versioned_hashes = versioned_hashes_from_commitments(
                &envelope.envelope_inner.blobs_bundle.commitments,
            );
            let cancun_fields = CancunPayloadFields {
                parent_beacon_block_root: parent_beacon_block_root
                    .ok_or_eyre("parent_beacon_block_root required for V4")?,
                versioned_hashes,
            };
            let prague_fields = PraguePayloadFields::new(envelope.execution_requests);
            Ok((
                ExecutionPayload::V3(envelope.envelope_inner.execution_payload),
                ExecutionPayloadSidecar::v4(cancun_fields, prague_fields),
            ))
        }
        5 => {
            // V5 (Osaka) - use raw request since alloy doesn't have get_payload_v5 yet
            use alloy_provider::Provider;
            use alloy_rpc_types_engine::ExecutionPayloadEnvelopeV5;

            let envelope: ExecutionPayloadEnvelopeV5 =
                provider.client().request(method, (payload_id,)).await?;
            let versioned_hashes =
                versioned_hashes_from_commitments(&envelope.blobs_bundle.commitments);
            let cancun_fields = CancunPayloadFields {
                parent_beacon_block_root: parent_beacon_block_root
                    .ok_or_eyre("parent_beacon_block_root required for V5")?,
                versioned_hashes,
            };
            let prague_fields = PraguePayloadFields::new(envelope.execution_requests);
            Ok((
                ExecutionPayload::V3(envelope.execution_payload),
                ExecutionPayloadSidecar::v4(cancun_fields, prague_fields),
            ))
        }
        _ => unreachable!(),
    }
}
