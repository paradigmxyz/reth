use crate::utils::{advance_with_random_transactions, EngineSszRequestExt, EngineSszResponseExt};
use alloy_eips::eip7685::RequestsOrHash;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{
    ClientVersionV1, ForkchoiceState, PayloadAttributes, PayloadStatusEnum,
};
use jsonrpsee_core::client::ClientT;
use reth_chainspec::{EthChainSpec, EthereumHardfork};
use reth_e2e_test_utils::{
    eth_payload_attributes, test_chain_spec, transaction::TransactionTestContext, wallet::Wallet,
    E2ETestSetupExt,
};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{
    node_config::NodeConfig,
    version::{version_metadata, CLIENT_CODE},
};
use reth_node_ethereum::{
    engine_ssz_containers::{
        BlobsV1Request, BlobsV1Response, BlobsV2Response, BlobsV3Response, BlobsV4Request,
        BlobsV4Response, BodiesByHashRequest, BodiesResponsePrague,
        ExecutionPayloadEnvelopeAmsterdam, ForkchoiceUpdateResponse as SszForkchoiceUpdateResponse,
        PayloadStatus as SszPayloadStatus, PayloadStatusWithWitness,
    },
    EthereumAddOns, EthereumNode,
};
use reth_provider::{BlockNumReader, StateProviderFactory};
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
use reth_tasks::Runtime;
use ssz::Encode;
use std::sync::Arc;

const ENGINE_PRAGUE_FORK_HEADER: &str = "prague";
const ENGINE_PAYLOADS_ROUTE: &str = "/engine/v1/payloads";
const ENGINE_FORKCHOICE_ROUTE: &str = "/engine/v1/forkchoice";
const ENGINE_V1_BLOBS_ROUTE: &str = "/engine/v1/blobs/v1";
const ENGINE_CAPABILITIES_ROUTE: &str = "/engine/v1/capabilities";
const ENGINE_IDENTITY_ROUTE: &str = "/engine/v1/identity";

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Cancun))
        .build_single()
        .await?;
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;

    // make the node advance
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let payload = node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    node.assert_new_block(tx_hash, block_hash, block_number).await?;

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn can_run_eth_node_with_auth_engine_api_over_ipc() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Cancun))
        .with_rpc_modifier(|rpc| rpc.with_auth_ipc())
        .build_single()
        .await?;

    // Create dummy transfer tx
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;

    // make the node advance
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let payload = node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    node.assert_new_block(tx_hash, block_hash, block_number).await?;

    Ok(())
}

#[tokio::test]
#[cfg(unix)]
async fn test_failed_run_eth_node_with_no_auth_engine_api_over_ipc_opts() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, _) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Cancun))
        .build_single()
        .await?;

    // Ensure that the engine api client is not available
    let client = node.inner.engine_ipc_client().await;
    assert!(client.is_none(), "ipc auth should be disabled by default");

    Ok(())
}

#[tokio::test]
async fn test_engine_graceful_shutdown() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Cancun))
        .build_single()
        .await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    let payload = node.advance_block().await?;
    node.assert_new_block(tx_hash, payload.block().hash(), payload.block().number).await?;

    // Get block number before shutdown
    let block_before = node.inner.provider.best_block_number()?;
    assert_eq!(block_before, 1, "Expected 1 block before shutdown");

    // Verify block is NOT yet persisted to database
    let db_block_before = node.inner.provider.last_block_number()?;
    assert_eq!(db_block_before, 0, "Block should not be persisted yet");

    // Trigger graceful shutdown
    let done_rx = node
        .inner
        .add_ons_handle
        .engine_shutdown
        .shutdown()
        .expect("shutdown should return receiver");

    tokio::time::timeout(std::time::Duration::from_secs(2), done_rx)
        .await
        .expect("shutdown timed out")
        .expect("shutdown completion channel should not be closed");

    let db_block = node.inner.provider.last_block_number()?;
    assert_eq!(db_block, 1, "Database should have persisted block 1");

    Ok(())
}

#[tokio::test]
async fn test_testing_build_block_v1_osaka() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec(EthereumHardfork::Osaka);
    let genesis_hash = chain_spec.genesis_hash();
    let (node, wallet) = EthereumNode::test_setup(1, chain_spec.clone())
        .with_rpc_modifier(|rpc| {
            rpc.with_http_api(RpcModuleSelection::from([
                RethRpcModule::Eth,
                RethRpcModule::Testing,
            ]))
        })
        .build_single()
        .await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;

    let request = TestingBuildBlockRequestV1 {
        parent_block_hash: genesis_hash,
        payload_attributes: eth_payload_attributes(&chain_spec, chain_spec.genesis().timestamp + 1),
        transactions: vec![raw_tx],
        extra_data: None,
    };

    let envelope = node.testing_build_block_v1(request).await?;

    let engine_client = node.auth_server_handle().http_client();
    let payload = envelope.execution_payload.clone();
    let block_hash = payload.payload_inner.payload_inner.block_hash;

    let versioned_hashes: Vec<B256> = Vec::new();
    let parent_beacon_block_root = B256::ZERO;
    let execution_requests = RequestsOrHash::Requests(envelope.execution_requests);

    let status: alloy_rpc_types_engine::PayloadStatus = engine_client
        .request(
            "engine_newPayloadV4",
            (payload, versioned_hashes, parent_beacon_block_root, execution_requests),
        )
        .await?;
    assert_eq!(status.status, PayloadStatusEnum::Valid);

    node.update_forkchoice(genesis_hash, block_hash).await?;

    node.wait_block(1, block_hash, false).await?;

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_proxy_can_mine_block() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec(EthereumHardfork::Prague);
    let genesis_hash = chain_spec.genesis_hash();
    let (node, _) = EthereumNode::test_setup(1, chain_spec.clone())
        .with_rpc_modifier(|rpc| {
            rpc.with_http_api(RpcModuleSelection::from([
                RethRpcModule::Eth,
                RethRpcModule::Testing,
            ]))
        })
        .build_single()
        .await?;

    let wallets = Wallet::new(2).wallet_gen();
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallets[0].clone()).await;

    let envelope = node
        .testing_build_block_v1(TestingBuildBlockRequestV1 {
            parent_block_hash: genesis_hash,
            payload_attributes: eth_payload_attributes(
                &chain_spec,
                chain_spec.genesis().timestamp + 1,
            ),
            transactions: vec![raw_tx],
            extra_data: None,
        })
        .await?;

    let payload = envelope.execution_payload;
    let block_hash = payload.payload_inner.payload_inner.block_hash;
    let client = reqwest::Client::new();
    let auth = node.auth_server_handle();
    let auth_url = auth.http_url();

    for route in [ENGINE_CAPABILITIES_ROUTE, ENGINE_PAYLOADS_ROUTE] {
        let response = client.get(format!("{auth_url}{route}")).send().await?;
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    let capabilities_response = client
        .get(format!("{auth_url}{ENGINE_CAPABILITIES_ROUTE}"))
        .jwt(&auth)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?;
    assert_eq!(capabilities_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        capabilities_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let capabilities: serde_json::Value = capabilities_response.json().await?;
    assert_eq!(
        capabilities,
        serde_json::json!({
            "supported_forks": ["paris", "shanghai", "cancun", "prague", "osaka", "amsterdam"],
            "fork_scoped_endpoints": ["payloads", "forkchoice", "bodies", "payloads/witness"],
            "independently_versioned": {
                "blobs": ["v1", "v2", "v3", "v4"],
            },
            "unscoped_endpoints": ["capabilities", "identity"],
            "limits": {
                "bodies.max_count": 32,
                "blobs.max_versioned_hashes": 128,
                "payload.max_bytes": 67108864,
            },
        })
    );

    let identity_response = client
        .get(format!("{auth_url}{ENGINE_IDENTITY_ROUTE}"))
        .jwt(&auth)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await?;
    assert_eq!(identity_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        identity_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let identity: Vec<ClientVersionV1> = identity_response.json().await?;
    assert_eq!(
        identity,
        vec![ClientVersionV1 {
            code: CLIENT_CODE,
            name: version_metadata().name_client.to_string(),
            version: version_metadata().cargo_pkg_version.to_string(),
            commit: version_metadata().vergen_git_sha.to_string(),
        }]
    );

    let status: SszPayloadStatus = client
        .post(format!("{auth_url}{ENGINE_PAYLOADS_ROUTE}"))
        .jwt(&auth)
        .fork(ENGINE_PRAGUE_FORK_HEADER)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .ssz(&(payload, B256::ZERO, envelope.execution_requests.take()))
        .send()
        .await?
        .ssz()
        .await?;
    assert_eq!(status.status, PayloadStatusEnum::Valid);

    let fcu: SszForkchoiceUpdateResponse = client
        .post(format!("{auth_url}{ENGINE_FORKCHOICE_ROUTE}"))
        .jwt(&auth)
        .fork(ENGINE_PRAGUE_FORK_HEADER)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .ssz(&(
            ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: genesis_hash,
                finalized_block_hash: genesis_hash,
            },
            Vec::<PayloadAttributes>::new(),
        ))
        .send()
        .await?
        .ssz()
        .await?;
    assert_eq!(fcu.payload_status.status, PayloadStatusEnum::Valid);

    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, wallets[1].clone()).await?;
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    let versioned_hashes = TransactionTestContext::validate_sidecar(envelope);

    let mut requested_hashes = versioned_hashes.clone();
    requested_hashes.push(B256::ZERO);
    let blobs: BlobsV1Response = client
        .post(format!("{auth_url}{ENGINE_V1_BLOBS_ROUTE}"))
        .jwt(&auth)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .ssz(&BlobsV1Request { versioned_hashes: requested_hashes })
        .send()
        .await?
        .ssz()
        .await?;
    assert_eq!(blobs.entries.len(), versioned_hashes.len() + 1);
    assert!(blobs.entries[..versioned_hashes.len()].iter().all(|entry| entry.available));
    assert!(!blobs.entries.last().unwrap().available);

    node.wait_block(1, block_hash, false).await?;

    for (fork, available) in [("prague", true), ("cancun", false)] {
        let bodies: BodiesResponsePrague = client
            .post(format!("{auth_url}/engine/v1/bodies/hash"))
            .jwt(&auth)
            .fork(fork)
            .ssz(&BodiesByHashRequest { block_hashes: vec![block_hash, B256::ZERO] })
            .send()
            .await?
            .ssz()
            .await?;
        assert_eq!(bodies.entries.len(), 2);
        assert_eq!(bodies.entries[0].available, available);
        assert!(!bodies.entries[1].available);
    }
    let bodies: BodiesResponsePrague = client
        .get(format!("{auth_url}/engine/v1/bodies?from=1&count=32"))
        .jwt(&auth)
        .fork("prague")
        .send()
        .await?
        .ssz()
        .await?;
    assert_eq!(bodies.entries.len(), 1);
    assert!(bodies.entries[0].available);

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_proxy_blob_revisions() -> eyre::Result<()> {
    let (mut node, _) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Osaka))
        .build_single()
        .await?;
    node.advance_block().await?;
    let client = reqwest::Client::new();
    let auth = node.auth_server_handle();
    let auth_url = auth.http_url();
    // Missing blobs retain one unavailable entry, except V2's all-or-nothing response.
    for version in 2..=4 {
        let request = client.post(format!("{auth_url}/engine/v1/blobs/v{version}")).jwt(&auth);
        let request = if version == 4 {
            request.ssz(&BlobsV4Request {
                versioned_hashes: vec![B256::ZERO],
                indices_bitarray: alloy_primitives::B128::ZERO,
            })
        } else {
            request.ssz(&BlobsV1Request { versioned_hashes: vec![B256::ZERO] })
        };
        let response = request.send().await?;
        if version == 2 {
            assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
            assert!(response.bytes().await?.is_empty());
            continue
        }
        let availability = match version {
            3 => response
                .ssz::<BlobsV3Response>()
                .await?
                .entries
                .into_iter()
                .map(|entry| entry.available)
                .collect::<Vec<_>>(),
            4 => response
                .ssz::<BlobsV4Response>()
                .await?
                .entries
                .into_iter()
                .map(|entry| entry.available)
                .collect(),
            _ => unreachable!(),
        };
        assert_eq!(availability, [false]);
    }
    // A container with no requested hashes is a successful empty V2 response.
    let blobs: BlobsV2Response = client
        .post(format!("{auth_url}/engine/v1/blobs/v2"))
        .jwt(&auth)
        .ssz(&BlobsV1Request { versioned_hashes: vec![] })
        .send()
        .await?
        .ssz()
        .await?;
    assert!(blobs.entries.is_empty());

    for (body, status) in [
        (vec![0; 32], reqwest::StatusCode::BAD_REQUEST),
        (
            BlobsV1Request { versioned_hashes: vec![B256::ZERO; 129] }.as_ssz_bytes(),
            reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = client
            .post(format!("{auth_url}/engine/v1/blobs/v1"))
            .jwt(&auth)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await?;
        assert_eq!(response.status(), status);
        response.problem_type().await?;
    }

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_proxy_returns_canonical_witness() -> eyre::Result<()> {
    let chain_spec = test_chain_spec(EthereumHardfork::Amsterdam);
    let genesis_hash = chain_spec.genesis_hash();
    let (mut node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let payload = node.new_payload().await?;
    let envelope = payload.try_into_v6()?;
    let request = ExecutionPayloadEnvelopeAmsterdam {
        payload: envelope.execution_payload,
        parent_beacon_block_root: B256::ZERO,
        execution_requests: envelope.execution_requests,
    };
    let client = reqwest::Client::new();
    let auth = node.auth_server_handle();
    let url = format!("{}/engine/v1/payloads/witness", auth.http_url());
    let response = client.post(&url).fork("amsterdam").ssz(&request).send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    for fork in ["prague", "amsterdam"] {
        let response = client.post(&url).jwt(&auth).fork(fork).ssz(&request).send().await?;
        if fork == "prague" {
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            continue
        }
        let response = response.ssz::<PayloadStatusWithWitness>().await?;
        assert_eq!(response.payload_status.status, PayloadStatusEnum::Valid);
        let witness = response.witness.as_ref().expect("valid payload includes a witness");
        assert!(!witness.state.is_empty());
        assert!(witness.state.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(witness.codes.windows(2).all(|pair| pair[0] < pair[1]));
        let parent: alloy_consensus::Header = alloy_rlp::decode_exact(&witness.headers[0])?;
        assert_eq!(parent.hash_slow(), genesis_hash);
    }
    let mut invalid = request;
    invalid.payload.payload_inner.payload_inner.payload_inner.block_hash = B256::ZERO;
    let response: PayloadStatusWithWitness =
        client.post(&url).jwt(&auth).fork("amsterdam").ssz(&invalid).send().await?.ssz().await?;
    assert!(matches!(response.payload_status.status, PayloadStatusEnum::Invalid { .. }));
    assert!(response.witness.is_none());
    invalid.payload.payload_inner.payload_inner.payload_inner.transactions =
        vec![alloy_primitives::Bytes::from_static(&[2])];
    let response = client.post(&url).jwt(&auth).fork("amsterdam").ssz(&invalid).send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.problem_type().await?, "/engine-api/errors/invalid-body");
    Ok(())
}

/// Tests that the sparse trie pipeline can be shared with the payload builder.
///
/// Enables both `share_execution_cache_with_payload_builder` and
/// `share_sparse_trie_with_payload_builder`, then advances multiple blocks with random
/// transactions. Each FCU spawns a `StateRootHandle` that the payload builder uses for
/// incremental state root computation instead of blocking `state_root_with_updates()`.
///
/// The test validates that all blocks are successfully built and their state roots are
/// accepted by the engine (newPayload returns VALID).
#[tokio::test]
async fn test_share_sparse_trie_with_payload_builder() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, _) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Prague))
        .with_tree_config_modifier(|config| {
            config
                .with_share_execution_cache_with_payload_builder(true)
                .with_share_sparse_trie_with_payload_builder(true)
        })
        .build_single()
        .await?;
    let mut rng = rand::rng();

    let num_blocks = 5;
    advance_with_random_transactions(&mut node, num_blocks, &mut rng, true).await?;

    let best_block = node.inner.provider.best_block_number()?;
    assert_eq!(best_block, num_blocks as u64, "Expected {} blocks, got {}", num_blocks, best_block);

    Ok(())
}

/// Tests that sparse trie allocation reuse works correctly across consecutive blocks.
///
/// This test exercises the sparse trie allocation reuse path by:
/// 1. Starting a node with the state-root task enabled
/// 2. Advancing multiple consecutive blocks with random transactions
/// 3. Verifying that all blocks are successfully validated (state roots match)
///
/// Note: Trie structure reuse is currently disabled due to pruning creating blinded
/// nodes. The preserved trie's allocations are still reused to reduce memory overhead,
/// but the trie is cleared between blocks.
#[tokio::test]
async fn test_sparse_trie_reuse_across_blocks() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Use the state-root task with pruning enabled.
    let (mut node, _) = EthereumNode::test_setup(1, test_chain_spec(EthereumHardfork::Prague))
        .with_tree_config_modifier(|config| config.with_sparse_trie_prune_depth(2))
        .build_single()
        .await?;

    // Use a seeded RNG for reproducibility
    let mut rng = rand::rng();

    // Advance multiple consecutive blocks with random transactions.
    // This exercises the sparse trie reuse path where each block's pruned trie
    // is reused for the next block's state root computation.
    let num_blocks = 5;
    advance_with_random_transactions(&mut node, num_blocks, &mut rng, true).await?;

    // Verify the chain advanced correctly
    let best_block = node.inner.provider.best_block_number()?;
    assert_eq!(best_block, num_blocks as u64, "Expected {} blocks, got {}", num_blocks, best_block);

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_request_validation() -> eyre::Result<()> {
    use alloy_primitives::Bytes;
    use reth_node_ethereum::engine_ssz_containers::{
        BuiltPayloadPrague, ExecutionPayloadEnvelopePrague, ExecutionPayloadPrague,
        ForkchoiceUpdateCancun, Optional, PayloadAttributesCancun,
    };

    let chain = test_chain_spec(EthereumHardfork::Prague);
    let (node, _) = EthereumNode::test_setup(1, chain.clone()).build_single().await?;
    let auth = node.auth_server_handle();
    let url = auth.http_url();
    let client = reqwest::Client::new();
    let state = ForkchoiceState {
        head_block_hash: chain.genesis_hash(),
        safe_block_hash: chain.genesis_hash(),
        finalized_block_hash: chain.genesis_hash(),
    };
    for (fork, withdrawals, expected_error) in [
        ("cancun", 0, Some("unsupported-fork")),
        ("osaka", 0, Some("unsupported-fork")),
        ("prague", 17, Some("ssz-decode-error")),
        ("prague", 16, None),
    ] {
        let attrs = PayloadAttributesCancun {
            timestamp: chain.genesis().timestamp + 1,
            withdrawals: vec![Default::default(); withdrawals],
            ..Default::default()
        };
        let response = client
            .post(format!("{url}{ENGINE_FORKCHOICE_ROUTE}"))
            .jwt(&auth)
            .fork(fork)
            .ssz(&ForkchoiceUpdateCancun {
                forkchoice_state: state,
                payload_attributes: Optional::some(attrs),
            })
            .send()
            .await?;
        if let Some(error) = expected_error {
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            assert_eq!(response.problem_type().await?, format!("/engine-api/errors/{error}"));
        } else {
            let fcu = response.ssz::<SszForkchoiceUpdateResponse>().await?;
            assert!(matches!(fcu.payload_status.status, PayloadStatusEnum::Valid));
            let id = fcu.payload_id.into_option().unwrap();
            let built: BuiltPayloadPrague = client
                .get(format!("{url}{ENGINE_PAYLOADS_ROUTE}/{id}"))
                .jwt(&auth)
                .fork(fork)
                .send()
                .await?
                .ssz()
                .await?;
            assert_eq!(built.payload.payload_inner.withdrawals.len(), 16);
        }
    }

    // The timestamp restriction only applies to builds, not historical head updates.
    let response = client
        .post(format!("{url}{ENGINE_FORKCHOICE_ROUTE}"))
        .jwt(&auth)
        .fork("cancun")
        .ssz(&ForkchoiceUpdateCancun {
            forkchoice_state: state,
            payload_attributes: Optional::none(),
        })
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    for (extra_data_len, transactions, status, error) in [
        (33, vec![], reqwest::StatusCode::BAD_REQUEST, "ssz-decode-error"),
        (
            0,
            vec![Bytes::from_static(&[2])],
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "invalid-body",
        ),
    ] {
        let mut payload = ExecutionPayloadPrague {
            payload_inner: alloy_rpc_types_engine::ExecutionPayloadV2 {
                payload_inner: alloy_rpc_types_engine::ExecutionPayloadV1::from_block_unchecked(
                    B256::ZERO,
                    &reth_ethereum_primitives::Block::default(),
                ),
                withdrawals: vec![],
            },
            blob_gas_used: 0,
            excess_blob_gas: 0,
        };
        payload.payload_inner.payload_inner.extra_data = vec![0; extra_data_len].into();
        payload.payload_inner.payload_inner.transactions = transactions;
        let response = client
            .post(format!("{url}{ENGINE_PAYLOADS_ROUTE}"))
            .jwt(&auth)
            .fork("prague")
            .ssz(&ExecutionPayloadEnvelopePrague {
                payload,
                parent_beacon_block_root: B256::ZERO,
                execution_requests: Default::default(),
            })
            .send()
            .await?;
        assert_eq!(response.status(), status);
        assert_eq!(response.problem_type().await?, format!("/engine-api/errors/{error}"));
    }
    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_custom_engine_and_middleware() -> eyre::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let middleware =
        tower::util::MapRequestLayer::new(move |request: jsonrpsee::server::HttpRequest| {
            if request.uri().path().starts_with("/engine/") {
                observed.fetch_add(1, Ordering::Relaxed);
            }
            request
        });
    let chain = test_chain_spec(EthereumHardfork::Prague);
    let NodeHandle { node, .. } =
        NodeBuilder::new(NodeConfig::test().with_chain(chain).with_unused_ports())
            .testing_node(Runtime::test())
            .with_types::<EthereumNode>()
            .with_components(EthereumNode::components())
            .with_add_ons(
                EthereumAddOns::default()
                    .with_engine_api(reth_node_builder::rpc::NoopEngineApiBuilder::default())
                    .layer_auth_http_middleware(middleware),
            )
            .launch()
            .await?;
    let auth = &node.add_ons_handle.rpc_server_handles.auth;
    let client = reqwest::Client::new();
    for route in [ENGINE_CAPABILITIES_ROUTE, ENGINE_IDENTITY_ROUTE] {
        let response = client.get(format!("{}{route}", auth.http_url())).jwt(auth).send().await?;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        assert_eq!(response.problem_type().await?, "/engine-api/errors/method-not-found");
    }
    assert_eq!(requests.load(Ordering::Relaxed), 2);
    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_witness_omitted_without_provider_parent_state() -> eyre::Result<()> {
    let (mut nodes, _) =
        EthereumNode::test_setup(2, test_chain_spec(EthereumHardfork::Amsterdam)).build().await?;
    let target = nodes.pop().unwrap();
    let mut source = nodes.pop().unwrap();
    let first = source.advance_block().await?;
    target.submit_payload(first).await?;
    let parent = source.advance_block().await?;
    let parent_hash = target.submit_payload(parent).await?;
    assert!(target.inner.provider.state_by_block_hash(parent_hash).is_err());
    let child = source.new_payload().await?.try_into_v6()?;
    let request = ExecutionPayloadEnvelopeAmsterdam {
        payload: child.execution_payload,
        parent_beacon_block_root: B256::ZERO,
        execution_requests: child.execution_requests,
    };
    let auth = target.auth_server_handle();
    let response: PayloadStatusWithWitness = reqwest::Client::new()
        .post(format!("{}/engine/v1/payloads/witness", auth.http_url()))
        .jwt(&auth)
        .fork("amsterdam")
        .ssz(&request)
        .send()
        .await?
        .ssz()
        .await?;
    assert_eq!(response.payload_status.status, PayloadStatusEnum::Valid);
    assert!(response.witness.is_none());
    Ok(())
}
