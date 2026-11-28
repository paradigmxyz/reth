//! EIP-8288 payload RPC, validation and durable proof storage.

use alloy_eips::eip7685::RequestsOrHash;
use alloy_primitives::{keccak256, Bytes, B256, U64};
use alloy_rpc_types_engine::{
    ExecutionPayloadBodiesV3, ExecutionPayloadEnvelopeV7, ForkchoiceState,
    ForkchoiceUpdatedResponseV2, PayloadStatusV2, MAX_DEPENDENCY_PROOF_BYTES,
};
use reth_chainspec::EthereumHardfork;
use reth_db::{tables, transaction::DbTxMut};
use reth_e2e_test_utils::{eth_payload_attributes, test_chain_spec, E2ETestSetupExt};
use reth_node_ethereum::EthereumNode;
use reth_provider::{DBProvider, DatabaseProviderFactory, DependencyProofProvider};
use serde_json::{json, Value};

#[tokio::test]
async fn payload_proofs_roundtrip_validate_and_survive_restart() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let spec = test_chain_spec(EthereumHardfork::Bogota);
    let (node, _) =
        EthereumNode::test_setup(1, spec.clone()).with_restartable_nodes().build_single().await?;
    let engine = node.auth_server_handle().http_client();
    let genesis = node.block_hash(0);
    let state = ForkchoiceState {
        head_block_hash: genesis,
        safe_block_hash: genesis,
        finalized_block_hash: genesis,
    };
    let mut attrs = eth_payload_attributes(&spec, 1);
    attrs.inclusion_lists = Some(vec![]);
    attrs.target_gas_limit = Some(30_000_000);
    let fcu = engine
        .request::<_, ForkchoiceUpdatedResponseV2>(
            "engine_forkchoiceUpdatedV6",
            (state, Some(attrs), Value::Null),
        )
        .await?;
    let id = fcu.payload_id.ok_or_else(|| eyre::eyre!("FCU V6 did not start a payload job"))?;
    let envelope =
        engine.request::<_, ExecutionPayloadEnvelopeV7>("engine_getPayloadV7", (id,)).await?;
    assert!(envelope.execution_payload.dependency_proof.is_empty());
    let payload = envelope.execution_payload;
    let v1 = &payload.payload_inner.payload_inner.payload_inner.payload_inner;
    let hash = v1.block_hash;
    let requests = RequestsOrHash::from(envelope.execution_requests);
    let mut missing = serde_json::to_value(&payload)?;
    missing.as_object_mut().unwrap().remove("dependencyProof");
    let missing = engine
        .request::<_, Value>(
            "engine_newPayloadV7",
            (missing, Vec::<B256>::new(), B256::ZERO, requests.clone(), Vec::<Value>::new()),
        )
        .await
        .unwrap_err();
    assert_eq!(missing.as_error_resp().unwrap().code, -32602);

    // Oversized proofs fail before block-hash computation.
    let mut oversized = payload.clone();
    oversized.dependency_proof = vec![0; MAX_DEPENDENCY_PROOF_BYTES + 1].into();
    let status = engine
        .request::<_, PayloadStatusV2>(
            "engine_newPayloadV7",
            (oversized, Vec::<Value>::new(), B256::ZERO, requests.clone(), Vec::<Value>::new()),
        )
        .await?;
    assert_eq!(
        status.payload_inner.status.validation_error(),
        Some(
            format!("dependency proof exceeds the byte limit: {}", MAX_DEPENDENCY_PROOF_BYTES + 1)
                .as_str()
        )
    );

    // Changed proof bytes with the old advertised hash fail the header commitment check.
    let mut changed = payload.clone();
    changed.dependency_proof = Bytes::from_static(&[1]);
    let status = engine
        .request::<_, PayloadStatusV2>(
            "engine_newPayloadV7",
            (
                changed.clone(),
                Vec::<Value>::new(),
                B256::ZERO,
                requests.clone(),
                Vec::<Value>::new(),
            ),
        )
        .await?;
    assert!(status.payload_inner.is_invalid());
    assert_eq!(status.payload_inner.latest_valid_hash, None);

    // Even a matching advertised hash cannot authorize a nonempty proof for empty dependencies.
    let built = node.payload.resolve_payload(id).await?;
    let mut header = built.block().header().clone();
    header.dependency_proof_hash = Some(keccak256(&changed.dependency_proof));
    changed.payload_inner.payload_inner.payload_inner.payload_inner.block_hash = header.hash_slow();
    let status = engine
        .request::<_, PayloadStatusV2>(
            "engine_newPayloadV7",
            (changed, Vec::<Value>::new(), B256::ZERO, requests.clone(), Vec::<Value>::new()),
        )
        .await?;
    assert_eq!(
        status.payload_inner.status.validation_error(),
        Some("invalid EIP-8288 aggregate proof")
    );
    drop(built);

    let status = engine
        .request::<_, PayloadStatusV2>(
            "engine_newPayloadV7",
            (payload.clone(), Vec::<Value>::new(), B256::ZERO, requests, Vec::<Value>::new()),
        )
        .await?;
    assert!(status.is_valid());
    assert_eq!(status.inclusion_list_satisfied, Some(true));
    assert_eq!(node.inner.provider.dependency_proof(hash)?, Some(Bytes::new()));
    let mut changed_known = payload.clone();
    changed_known.dependency_proof = Bytes::from_static(&[1]);
    let status = engine
        .request::<_, PayloadStatusV2>(
            "engine_newPayloadV7",
            (
                changed_known,
                Vec::<Value>::new(),
                B256::ZERO,
                RequestsOrHash::default(),
                Vec::<Value>::new(),
            ),
        )
        .await?;
    assert!(status.payload_inner.is_invalid());
    assert_eq!(status.payload_inner.latest_valid_hash, None);

    let old_version = engine
        .request::<_, PayloadStatusV2>(
            "engine_newPayloadV6",
            (
                payload.payload_inner,
                Vec::<Value>::new(),
                B256::ZERO,
                RequestsOrHash::default(),
                Vec::<Value>::new(),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(old_version.as_error_resp().unwrap().code, -38005);

    let state = ForkchoiceState {
        head_block_hash: hash,
        safe_block_hash: genesis,
        finalized_block_hash: genesis,
    };
    assert!(engine
        .request::<_, ForkchoiceUpdatedResponseV2>(
            "engine_forkchoiceUpdatedV6",
            (state, Value::Null, Value::Null,)
        )
        .await?
        .payload_status
        .is_valid());
    let bodies = engine
        .request::<_, ExecutionPayloadBodiesV3>(
            "engine_getPayloadBodiesByHashV3",
            (vec![B256::repeat_byte(99), genesis, hash],),
        )
        .await?;
    assert_eq!(bodies[0], None);
    assert_eq!(bodies[1].as_ref().unwrap().dependency_proof, None);
    assert_eq!(bodies[2].as_ref().unwrap().dependency_proof, Some(Bytes::new()));
    let range = engine
        .request::<_, ExecutionPayloadBodiesV3>(
            "engine_getPayloadBodiesByRangeV3",
            (U64::from(1), U64::from(2)),
        )
        .await?;
    assert_eq!(range, vec![bodies[2].clone()]);
    let error = engine
        .request::<_, ExecutionPayloadBodiesV3>(
            "engine_getPayloadBodiesByRangeV3",
            (U64::ZERO, U64::from(1)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.as_error_resp().unwrap().code, -32602);
    let error = engine
        .request::<_, ExecutionPayloadBodiesV3>(
            "engine_getPayloadBodiesByHashV3",
            (vec![hash; 1025],),
        )
        .await
        .unwrap_err();
    assert_eq!(error.as_error_resp().unwrap().code, -38004);
    drop(engine);

    let node = node.restart().await?;
    let engine = node.auth_server_handle().http_client();
    let bodies = engine
        .request::<_, ExecutionPayloadBodiesV3>(
            "engine_getPayloadBodiesByRangeV3",
            (U64::from(1), U64::from(1)),
        )
        .await?;
    assert_eq!(bodies[0].as_ref().unwrap().dependency_proof, Some(Bytes::new()));
    assert_eq!(serde_json::to_value(&bodies)?[0]["dependencyProof"], json!("0x"));

    // Simulate a separately pruned proof without deleting the block or inferring empty data.
    let provider = node.inner.provider.database_provider_rw()?;
    provider.tx_ref().delete::<tables::DependencyProofs>(hash, None)?;
    provider.commit()?;
    let bodies = engine
        .request::<_, ExecutionPayloadBodiesV3>("engine_getPayloadBodiesByHashV3", (vec![hash],))
        .await?;
    assert_eq!(bodies[0].as_ref().unwrap().dependency_proof, None);
    assert_eq!(serde_json::to_value(&bodies)?[0]["dependencyProof"], Value::Null);
    Ok(())
}
