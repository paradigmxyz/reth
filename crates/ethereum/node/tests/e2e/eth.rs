use crate::utils::{
    advance_with_random_transactions, eth_payload_attributes, eth_payload_attributes_amsterdam,
};
use alloy_eips::eip7685::RequestsOrHash;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::{
    ClientVersionV1, ForkchoiceState, PayloadAttributes, PayloadStatusEnum,
};
use jsonrpsee_core::client::ClientT;
use reth_chainspec::{ChainSpecBuilder, EthChainSpec, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, setup, setup_engine, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_api::TreeConfig;
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{
    args::RpcServerArgs,
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
use reth_rpc_layer::secret_to_bearer_header;
use reth_tasks::Runtime;
use ssz::{Decode, Encode};
use std::sync::Arc;

const ENGINE_EXECUTION_VERSION_HEADER: &str = "Eth-Execution-Version";
const ENGINE_PRAGUE_FORK_HEADER: &str = "prague";
const ENGINE_PAYLOADS_ROUTE: &str = "/engine/v1/payloads";
const ENGINE_FORKCHOICE_ROUTE: &str = "/engine/v1/forkchoice";
const ENGINE_V1_BLOBS_ROUTE: &str = "/engine/v1/blobs/v1";
const ENGINE_CAPABILITIES_ROUTE: &str = "/engine/v1/capabilities";
const ENGINE_IDENTITY_ROUTE: &str = "/engine/v1/identity";

#[tokio::test]
async fn can_run_eth_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, wallet) = setup::<EthereumNode>(
        1,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ),
        false,
        eth_payload_attributes,
    )
    .await?;

    let mut node = nodes.pop().unwrap();
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
    let runtime = Runtime::test();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    // Node setup
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http().with_auth_ipc());

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;
    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    // Configure wallet from test mnemonic and create dummy transfer tx
    let wallet = Wallet::default();
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
    let runtime = Runtime::test();

    // Chain spec with test allocs
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    // Node setup
    let node_config = NodeConfig::test().with_chain(chain_spec);
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    let node = NodeTestContext::new(node, eth_payload_attributes).await?;

    // Ensure that the engine api client is not available
    let client = node.inner.engine_ipc_client().await;
    assert!(client.is_none(), "ipc auth should be disabled by default");

    Ok(())
}

#[tokio::test]
async fn test_engine_graceful_shutdown() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, wallet) = setup::<EthereumNode>(
        1,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ),
        false,
        eth_payload_attributes,
    )
    .await?;

    let mut node = nodes.pop().unwrap();

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
    let runtime = Runtime::test();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default().chain(MAINNET.chain).genesis(genesis).osaka_activated().build(),
    );
    let genesis_hash = chain_spec.genesis_hash();

    let node_config =
        NodeConfig::test().with_chain(chain_spec.clone()).with_unused_ports().with_rpc(
            RpcServerArgs::default()
                .with_unused_ports()
                .with_http()
                .with_http_api(reth_rpc_server_types::RpcModuleSelection::All),
        );

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    let node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallet = Wallet::default();
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;

    let payload_attributes = PayloadAttributes {
        timestamp: chain_spec.genesis().timestamp + 1,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
        slot_number: None,
        ..Default::default()
    };

    let request = TestingBuildBlockRequestV1 {
        parent_block_hash: genesis_hash,
        payload_attributes,
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
    let runtime = Runtime::test();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .prague_activated()
            .build(),
    );
    let genesis_hash = chain_spec.genesis_hash();
    let node_config =
        NodeConfig::test().with_chain(chain_spec.clone()).with_unused_ports().with_rpc(
            RpcServerArgs::default()
                .with_unused_ports()
                .with_http()
                .with_http_api(reth_rpc_server_types::RpcModuleSelection::All),
        );

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch()
        .await?;

    let node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallets = Wallet::new(2).wallet_gen();
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallets[0].clone()).await;

    let payload_attributes = PayloadAttributes {
        timestamp: chain_spec.genesis().timestamp + 1,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
        slot_number: None,
        ..Default::default()
    };

    let envelope = node
        .testing_build_block_v1(TestingBuildBlockRequestV1 {
            parent_block_hash: genesis_hash,
            payload_attributes,
            transactions: vec![raw_tx],
            extra_data: None,
        })
        .await?;

    let payload = envelope.execution_payload;
    let block_hash = payload.payload_inner.payload_inner.block_hash;
    let client = reqwest::Client::new();
    let auth_server = node.auth_server_handle();
    let auth_url = auth_server.http_url();
    let auth_header = secret_to_bearer_header(auth_server.jwt_secret());

    for route in [ENGINE_CAPABILITIES_ROUTE, ENGINE_PAYLOADS_ROUTE] {
        let response = client.get(format!("{auth_url}{route}")).send().await?;
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    let capabilities_response = client
        .get(format!("{auth_url}{ENGINE_CAPABILITIES_ROUTE}"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
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
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
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

    let new_payload_response = client
        .post(format!("{auth_url}{ENGINE_PAYLOADS_ROUTE}"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(ENGINE_EXECUTION_VERSION_HEADER, ENGINE_PRAGUE_FORK_HEADER)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .body((payload, B256::ZERO, envelope.execution_requests.take()).as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(new_payload_response.status(), reqwest::StatusCode::OK);

    let status = SszPayloadStatus::from_ssz_bytes(&new_payload_response.bytes().await?).unwrap();
    assert_eq!(status.status, PayloadStatusEnum::Valid);

    let fcu_response = client
        .post(format!("{auth_url}{ENGINE_FORKCHOICE_ROUTE}"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(ENGINE_EXECUTION_VERSION_HEADER, ENGINE_PRAGUE_FORK_HEADER)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .body(
            (
                ForkchoiceState {
                    head_block_hash: block_hash,
                    safe_block_hash: genesis_hash,
                    finalized_block_hash: genesis_hash,
                },
                Vec::<PayloadAttributes>::new(),
            )
                .as_ssz_bytes(),
        )
        .send()
        .await?;
    assert_eq!(fcu_response.status(), reqwest::StatusCode::OK);

    let blob_tx = TransactionTestContext::tx_with_blobs_bytes(1, wallets[1].clone()).await?;
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    let versioned_hashes = TransactionTestContext::validate_sidecar(envelope);

    let mut requested_hashes = versioned_hashes.clone();
    requested_hashes.push(B256::ZERO);
    let blobs_response = client
        .post(format!("{auth_url}{ENGINE_V1_BLOBS_ROUTE}"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .body(BlobsV1Request { versioned_hashes: requested_hashes }.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(blobs_response.status(), reqwest::StatusCode::OK);

    let blobs = BlobsV1Response::from_ssz_bytes(&blobs_response.bytes().await?).unwrap();
    assert_eq!(blobs.entries.len(), versioned_hashes.len() + 1);
    assert!(blobs.entries[..versioned_hashes.len()].iter().all(|entry| entry.available));
    assert!(!blobs.entries.last().unwrap().available);

    let fcu = SszForkchoiceUpdateResponse::from_ssz_bytes(&fcu_response.bytes().await?).unwrap();
    assert_eq!(fcu.payload_status.status, PayloadStatusEnum::Valid);

    node.wait_block(1, block_hash, false).await?;

    for (fork, available) in [("prague", true), ("cancun", false)] {
        let response = client
            .post(format!("{auth_url}/engine/v1/bodies/hash"))
            .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
            .header(ENGINE_EXECUTION_VERSION_HEADER, fork)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(BodiesByHashRequest { block_hashes: vec![block_hash, B256::ZERO] }.as_ssz_bytes())
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let bodies = BodiesResponsePrague::from_ssz_bytes(&response.bytes().await?).unwrap();
        assert_eq!(bodies.entries.len(), 2);
        assert_eq!(bodies.entries[0].available, available);
        assert!(!bodies.entries[1].available);
    }
    let response = client
        .get(format!("{auth_url}/engine/v1/bodies?from=1&count=32"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(ENGINE_EXECUTION_VERSION_HEADER, "prague")
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let bodies = BodiesResponsePrague::from_ssz_bytes(&response.bytes().await?).unwrap();
    assert_eq!(bodies.entries.len(), 1);
    assert!(bodies.entries[0].available);

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_proxy_blob_revisions() -> eyre::Result<()> {
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .osaka_activated()
            .build(),
    );
    let (mut nodes, _) =
        setup::<EthereumNode>(1, chain_spec, false, eth_payload_attributes).await?;
    let mut node = nodes.pop().unwrap();
    node.advance_block().await?;
    let client = reqwest::Client::new();
    let auth_server = node.auth_server_handle();
    let auth_url = auth_server.http_url();
    let auth_header = secret_to_bearer_header(auth_server.jwt_secret());
    // Missing blobs retain one unavailable entry, except V2's all-or-nothing response.
    for version in 2..=4 {
        let body = if version == 4 {
            BlobsV4Request {
                versioned_hashes: vec![B256::ZERO],
                indices_bitarray: alloy_primitives::B128::ZERO,
            }
            .as_ssz_bytes()
        } else {
            BlobsV1Request { versioned_hashes: vec![B256::ZERO] }.as_ssz_bytes()
        };
        let response = client
            .post(format!("{auth_url}/engine/v1/blobs/v{version}"))
            .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await?;
        if version == 2 {
            assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
            assert!(response.bytes().await?.is_empty());
            continue
        }
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let bytes = response.bytes().await?;
        let availability = match version {
            3 => BlobsV3Response::from_ssz_bytes(&bytes)
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| entry.available)
                .collect::<Vec<_>>(),
            4 => BlobsV4Response::from_ssz_bytes(&bytes)
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| entry.available)
                .collect(),
            _ => unreachable!(),
        };
        assert_eq!(availability, [false]);
    }
    // A container with no requested hashes is a successful empty V2 response.
    let response = client
        .post(format!("{auth_url}/engine/v1/blobs/v2"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(BlobsV1Request { versioned_hashes: vec![] }.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(BlobsV2Response::from_ssz_bytes(&response.bytes().await?).unwrap().entries.is_empty());

    for (body, status) in [
        (vec![0; 32], reqwest::StatusCode::BAD_REQUEST),
        (
            BlobsV1Request { versioned_hashes: vec![B256::ZERO; 129] }.as_ssz_bytes(),
            reqwest::StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = client
            .post(format!("{auth_url}/engine/v1/blobs/v1"))
            .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await?;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[reqwest::header::CONTENT_TYPE], "application/problem+json");
    }

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_proxy_returns_canonical_witness() -> eyre::Result<()> {
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .amsterdam_activated()
            .build(),
    );
    let genesis_hash = chain_spec.genesis_hash();
    let (mut nodes, _) =
        setup::<EthereumNode>(1, chain_spec, false, eth_payload_attributes_amsterdam).await?;
    let mut node = nodes.pop().unwrap();
    let payload = node.new_payload().await?;
    let envelope = payload.try_into_v6()?;
    let request = ExecutionPayloadEnvelopeAmsterdam {
        payload: envelope.execution_payload,
        parent_beacon_block_root: B256::ZERO,
        execution_requests: envelope.execution_requests,
    };
    let client = reqwest::Client::new();
    let auth_server = node.auth_server_handle();
    let auth_url = auth_server.http_url();
    let auth_header = secret_to_bearer_header(auth_server.jwt_secret());
    let url = format!("{auth_url}/engine/v1/payloads/witness");
    let response = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(ENGINE_EXECUTION_VERSION_HEADER, "amsterdam")
        .body(request.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    for fork in ["prague", "amsterdam"] {
        let response = client
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(ENGINE_EXECUTION_VERSION_HEADER, fork)
            .body(request.as_ssz_bytes())
            .send()
            .await?;
        if fork == "prague" {
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            continue
        }
        let status = response.status();
        let bytes = response.bytes().await?;
        assert_eq!(status, reqwest::StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
        let response = PayloadStatusWithWitness::from_ssz_bytes(&bytes).unwrap();
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
    let response = client
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(ENGINE_EXECUTION_VERSION_HEADER, "amsterdam")
        .body(invalid.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response = PayloadStatusWithWitness::from_ssz_bytes(&response.bytes().await?).unwrap();
    assert!(matches!(response.payload_status.status, PayloadStatusEnum::Invalid { .. }));
    assert!(response.witness.is_none());
    invalid.payload.payload_inner.payload_inner.payload_inner.transactions =
        vec![alloy_primitives::Bytes::from_static(&[2])];
    let response = client
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(ENGINE_EXECUTION_VERSION_HEADER, "amsterdam")
        .body(invalid.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.json::<serde_json::Value>().await?["type"],
        "/engine-api/errors/invalid-body"
    );
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

    let tree_config = TreeConfig::default()
        .with_share_execution_cache_with_payload_builder(true)
        .with_share_sparse_trie_with_payload_builder(true);

    let (mut nodes, _wallet) = setup_engine::<EthereumNode>(
        1,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .prague_activated()
                .build(),
        ),
        false,
        tree_config,
        eth_payload_attributes,
    )
    .await?;

    let mut node = nodes.pop().unwrap();
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
    let tree_config = TreeConfig::default().with_sparse_trie_prune_depth(2);

    let (mut nodes, _wallet) = setup_engine::<EthereumNode>(
        1,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .prague_activated()
                .build(),
        ),
        false,
        tree_config,
        eth_payload_attributes,
    )
    .await?;

    let mut node = nodes.pop().unwrap();

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

    let chain = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .prague_activated()
            .build(),
    );
    let (mut nodes, _) =
        setup::<EthereumNode>(1, chain.clone(), false, eth_payload_attributes).await?;
    let node = nodes.pop().unwrap();
    let auth = node.auth_server_handle();
    let url = auth.http_url();
    let jwt = secret_to_bearer_header(auth.jwt_secret());
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
            .header("Authorization", jwt.to_str()?)
            .header("Content-Type", "application/octet-stream")
            .header(ENGINE_EXECUTION_VERSION_HEADER, fork)
            .body(
                ForkchoiceUpdateCancun {
                    forkchoice_state: state,
                    payload_attributes: Optional::some(attrs),
                }
                .as_ssz_bytes(),
            )
            .send()
            .await?;
        if let Some(error) = expected_error {
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            assert_eq!(
                response.json::<serde_json::Value>().await?["type"],
                format!("/engine-api/errors/{error}")
            );
        } else {
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            let fcu =
                SszForkchoiceUpdateResponse::from_ssz_bytes(&response.bytes().await?).unwrap();
            assert!(matches!(fcu.payload_status.status, PayloadStatusEnum::Valid));
            let id = fcu.payload_id.into_option().unwrap();
            let response = client
                .get(format!("{url}{ENGINE_PAYLOADS_ROUTE}/{id}"))
                .header("Authorization", jwt.to_str()?)
                .header(ENGINE_EXECUTION_VERSION_HEADER, fork)
                .send()
                .await?;
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            let built = BuiltPayloadPrague::from_ssz_bytes(&response.bytes().await?).unwrap();
            assert_eq!(built.payload.payload_inner.withdrawals.len(), 16);
        }
    }

    // The timestamp restriction only applies to builds, not historical head updates.
    let response = client
        .post(format!("{url}{ENGINE_FORKCHOICE_ROUTE}"))
        .header("Authorization", jwt.to_str()?)
        .header("Content-Type", "application/octet-stream")
        .header(ENGINE_EXECUTION_VERSION_HEADER, "cancun")
        .body(
            ForkchoiceUpdateCancun {
                forkchoice_state: state,
                payload_attributes: Optional::none(),
            }
            .as_ssz_bytes(),
        )
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
            .header("Authorization", jwt.to_str()?)
            .header("Content-Type", "application/octet-stream")
            .header(ENGINE_EXECUTION_VERSION_HEADER, "prague")
            .body(
                ExecutionPayloadEnvelopePrague {
                    payload,
                    parent_beacon_block_root: B256::ZERO,
                    execution_requests: Default::default(),
                }
                .as_ssz_bytes(),
            )
            .send()
            .await?;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[reqwest::header::CONTENT_TYPE], "application/problem+json");
        assert_eq!(
            response.json::<serde_json::Value>().await?["type"],
            format!("/engine-api/errors/{error}")
        );
    }
    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_custom_engine_and_middleware() -> eyre::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let runtime = Runtime::test();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let middleware =
        tower::util::MapRequestLayer::new(move |request: jsonrpsee::server::HttpRequest| {
            if request.uri().path().starts_with("/engine/") {
                observed.fetch_add(1, Ordering::Relaxed);
            }
            request
        });
    let chain = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .prague_activated()
            .build(),
    );
    let NodeHandle { node, .. } =
        NodeBuilder::new(NodeConfig::test().with_chain(chain).with_unused_ports())
            .testing_node(runtime)
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
    let jwt = secret_to_bearer_header(auth.jwt_secret());
    let client = reqwest::Client::new();
    for route in [ENGINE_CAPABILITIES_ROUTE, ENGINE_IDENTITY_ROUTE] {
        let response = client
            .get(format!("{}{route}", auth.http_url()))
            .header("Authorization", jwt.to_str()?)
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        assert_eq!(
            response.json::<serde_json::Value>().await?["type"],
            "/engine-api/errors/method-not-found"
        );
    }
    assert_eq!(requests.load(Ordering::Relaxed), 2);
    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_witness_omitted_without_provider_parent_state() -> eyre::Result<()> {
    let chain = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json"))?)
            .amsterdam_activated()
            .build(),
    );
    let (mut nodes, _) =
        setup::<EthereumNode>(2, chain, false, eth_payload_attributes_amsterdam).await?;
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
    let jwt = secret_to_bearer_header(auth.jwt_secret());
    let response = reqwest::Client::new()
        .post(format!("{}/engine/v1/payloads/witness", auth.http_url()))
        .header("Authorization", jwt.to_str()?)
        .header("Content-Type", "application/octet-stream")
        .header(ENGINE_EXECUTION_VERSION_HEADER, "amsterdam")
        .body(request.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response = PayloadStatusWithWitness::from_ssz_bytes(&response.bytes().await?).unwrap();
    assert_eq!(response.payload_status.status, PayloadStatusEnum::Valid);
    assert!(response.witness.is_none());
    Ok(())
}
