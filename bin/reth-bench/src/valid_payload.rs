//! This is an extension trait for any provider that implements the engine API, to wait for a VALID
//! response. This is useful for benchmarking, as it allows us to wait for a payload to be valid
//! before sending additional calls.

use alloy_consensus::TxEnvelope;
use alloy_primitives::Bytes;
use alloy_provider::{ext::EngineApi, network::AnyRpcBlock, Network, Provider};
use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayload, ExecutionPayloadInputV2, ExecutionPayloadSidecar,
    ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use alloy_transport::TransportResult;
use reth_node_api::EngineApiMessageVersion;
use reth_node_core::args::WaitForPersistence;
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
///
/// `wait_for_persistence` controls how `wait_for_persistence` is passed to
/// `reth_newPayload` on a per-block basis.
pub(crate) fn block_to_new_payload(
    block: AnyRpcBlock,
    rlp: Option<Bytes>,
    reth_new_payload: bool,
    wait_for_persistence: WaitForPersistence,
    no_wait_for_caches: bool,
) -> eyre::Result<(Option<EngineApiMessageVersion>, serde_json::Value)> {
    let block_number = block.header.number;
    let wait_for_persistence = wait_for_persistence.rpc_value(block_number);

    if let Some(rlp) = rlp {
        return Ok((
            None,
            serde_json::to_value((
                RethNewPayloadInput::<ExecutionData>::BlockRlp(rlp),
                wait_for_persistence,
                no_wait_for_caches.then_some(false),
            ))?,
        ));
    }

    let (payload, sidecar) = ethereum_block_to_payload(block)?;
    let (version, params, execution_data) = payload_to_new_payload(payload, sidecar, None)?;

    if reth_new_payload {
        Ok((
            None,
            serde_json::to_value((
                RethNewPayloadInput::ExecutionData(execution_data),
                wait_for_persistence,
                no_wait_for_caches.then_some(false),
            ))?,
        ))
    } else {
        Ok((Some(version), params))
    }
}

fn ethereum_block_to_payload(
    block: AnyRpcBlock,
) -> eyre::Result<(ExecutionPayload, ExecutionPayloadSidecar)> {
    let block = block
        .into_inner()
        .map_header(|header| header.map(|h| h.into_header_with_defaults()))
        .try_map_transactions(|tx| -> eyre::Result<TxEnvelope> {
            tx.try_into().map_err(|_| eyre::eyre!("unsupported tx type"))
        })?
        .into_consensus();

    Ok(ExecutionPayload::from_block_slow(&block))
}

/// Converts an execution payload and sidecar into versioned engine API params and an
/// [`ExecutionData`].
///
/// Returns `(version, versioned_params, execution_data)`.
pub(crate) fn payload_to_new_payload(
    payload: ExecutionPayload,
    sidecar: ExecutionPayloadSidecar,
    target_version: Option<EngineApiMessageVersion>,
) -> eyre::Result<(EngineApiMessageVersion, serde_json::Value, ExecutionData)> {
    let execution_data = ExecutionData { payload: payload.clone(), sidecar: sidecar.clone() };

    let (version, params) = match payload {
        ExecutionPayload::V3(payload) => {
            let cancun = sidecar
                .cancun()
                .ok_or_else(|| eyre::eyre!("missing cancun sidecar for V3 payload"))?;

            if let Some(prague) = sidecar.prague() {
                let version = target_version.unwrap_or(EngineApiMessageVersion::V4);
                let requests = prague.requests.clone();
                (
                    version,
                    serde_json::to_value((
                        payload,
                        cancun.versioned_hashes.clone(),
                        cancun.parent_beacon_block_root,
                        requests,
                    ))?,
                )
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

/// Calls the correct `engine_newPayload` method depending on the given execution payload and its
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7685::RequestsOrHash;
    use alloy_primitives::{Address, Bloom, B256, U256};
    use alloy_rpc_types_engine::{
        CancunPayloadFields, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
        PraguePayloadFields,
    };

    #[test]
    fn payload_to_new_payload_v3_uses_standard_ethereum_params() {
        let payload = sample_execution_payload_v3();
        let execution_payload = ExecutionPayload::V3(payload.clone());
        let parent_beacon_block_root = B256::from([0x11; 32]);
        let versioned_hash = B256::from([0x22; 32]);
        let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields::new(
            parent_beacon_block_root,
            vec![versioned_hash],
        ));

        let (version, params, _) =
            payload_to_new_payload(execution_payload, sidecar, None).expect("payload encodes");

        assert_eq!(version, EngineApiMessageVersion::V3);
        assert_eq!(
            params,
            serde_json::to_value((payload, vec![versioned_hash], parent_beacon_block_root))
                .expect("params serialize"),
        );
    }

    #[test]
    fn payload_to_new_payload_v4_preserves_requests_hash() {
        let payload = sample_execution_payload_v3();
        let execution_payload = ExecutionPayload::V3(payload.clone());
        let parent_beacon_block_root = B256::from([0x33; 32]);
        let versioned_hash = B256::from([0x44; 32]);
        let requests_hash = B256::from([0x55; 32]);
        let requests = RequestsOrHash::Hash(requests_hash);
        let sidecar = ExecutionPayloadSidecar::v4(
            CancunPayloadFields::new(parent_beacon_block_root, vec![versioned_hash]),
            PraguePayloadFields::new(requests.clone()),
        );

        let (version, params, _) =
            payload_to_new_payload(execution_payload, sidecar, None).expect("payload encodes");

        assert_eq!(version, EngineApiMessageVersion::V4);
        assert_eq!(
            params,
            serde_json::to_value((
                payload,
                vec![versioned_hash],
                parent_beacon_block_root,
                requests,
            ))
            .expect("params serialize"),
        );
    }

    #[test]
    fn ethereum_block_to_payload_rejects_unsupported_transaction_types() {
        let block = serde_json::from_value::<AnyRpcBlock>(serde_json::json!({
            "hash": "0xef664d656f841b5ad6a2b527b963f1eb48b97d7889d742f6cbff6950388e24cd",
            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
            "miner": "0x0000000000000000000000000000000000000000",
            "stateRoot": "0x5eb6e371a698b8d68f665192350ffcecbbbf322916f4b51bd79bb6887da3f494",
            "transactionsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            "receiptsRoot": "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "difficulty": "0x0",
            "number": "0x1",
            "gasLimit": "0x1c9c380",
            "gasUsed": "0x0",
            "timestamp": "0x1",
            "extraData": "0x",
            "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "nonce": "0x0000000000000000",
            "baseFeePerGas": "0x1",
            "transactions": [{
                "blockHash": "0xef664d656f841b5ad6a2b527b963f1eb48b97d7889d742f6cbff6950388e24cd",
                "blockNumber": "0x1",
                "depositReceiptVersion": "0x1",
                "from": "0x36bde71c97b33cc4729cf772ae268934f7ab70b2",
                "gas": "0x5208",
                "gasPrice": "0x1",
                "hash": "0x0bf1845c5d7a82ec92365d5027f7310793d53004f3c86aa80965c67bf7e7dc80",
                "input": "0x",
                "mint": "0x0",
                "nonce": "0x0",
                "r": "0x0",
                "s": "0x0",
                "sourceHash": "0x074adb22f2e6ed9bdd31c52eefc1f050e5db56eb85056450bccd79a6649520b3",
                "to": "0x4200000000000000000000000000000000000007",
                "transactionIndex": "0x0",
                "type": "0x7e",
                "v": "0x0",
                "value": "0x0"
            }],
            "uncles": []
        }))
        .expect("block deserializes");

        let err = ethereum_block_to_payload(block).expect_err("unsupported tx fails");
        assert!(err.to_string().contains("unsupported tx type"));
    }

    fn sample_execution_payload_v3() -> ExecutionPayloadV3 {
        ExecutionPayloadV3 {
            payload_inner: ExecutionPayloadV2 {
                payload_inner: ExecutionPayloadV1 {
                    parent_hash: B256::from([0x01; 32]),
                    fee_recipient: Address::from([0x02; 20]),
                    state_root: B256::from([0x03; 32]),
                    receipts_root: B256::from([0x04; 32]),
                    logs_bloom: Bloom::default(),
                    prev_randao: B256::from([0x05; 32]),
                    block_number: 42,
                    gas_limit: 30_000_000,
                    gas_used: 21_000,
                    timestamp: 1_715_555_555,
                    extra_data: Bytes::default(),
                    base_fee_per_gas: U256::from(7),
                    block_hash: B256::from([0x06; 32]),
                    transactions: vec![],
                },
                withdrawals: vec![],
            },
            blob_gas_used: 10,
            excess_blob_gas: 20,
        }
    }
}
