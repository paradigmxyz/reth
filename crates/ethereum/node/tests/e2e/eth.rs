use crate::utils::{
    advance_with_random_transactions, eth_payload_attributes, eth_payload_attributes_amsterdam,
};
use alloy_eips::{eip4844::BlobAndProofV1, eip7685::RequestsOrHash};
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
use reth_node_builder::{rpc::BasicEngineApiBuilder, EngineApiExt, NodeBuilder, NodeHandle};
use reth_node_core::{
    args::RpcServerArgs,
    node_config::NodeConfig,
    version::{version_metadata, CLIENT_CODE},
};
use reth_node_ethereum::{
    engine_ssz_containers::{
        ExecutionPayloadEnvelopeAmsterdam,
        ForkchoiceUpdateResponse as SszForkchoiceUpdateResponse, PayloadStatus as SszPayloadStatus,
        PayloadStatusWithWitness,
    },
    engine_ssz_proxy::{EngineSszProxyLayer, EngineSszWitnessGenerator},
    EthereumAddOns, EthereumEngineValidatorBuilder, EthereumNode,
};
use reth_provider::BlockNumReader;
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_layer::secret_to_bearer_header;
use reth_tasks::Runtime;
use ssz::{Decode, Encode};
use std::sync::Arc;

const ENGINE_EXECUTION_VERSION_HEADER: &str = "Eth-Execution-Version";
const ENGINE_PRAGUE_FORK_HEADER: &str = "prague";
const ENGINE_AMSTERDAM_FORK_HEADER: &str = "amsterdam";
const ENGINE_PAYLOADS_ROUTE: &str = "/engine/v1/payloads";
const ENGINE_PAYLOADS_WITNESS_ROUTE: &str = "/engine/v1/payloads/witness";
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
        target_gas_limit: None,
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

    let (ssz_layer, ssz_handle) = EngineSszProxyLayer::new();
    let engine_api_handle = ssz_handle.clone();
    let engine_api_builder = EngineApiExt::new(
        BasicEngineApiBuilder::<EthereumEngineValidatorBuilder>::default(),
        move |engine_api| {
            engine_api_handle.set_engine_api_sync(engine_api);
        },
    );
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(
            EthereumAddOns::default()
                .with_engine_api(engine_api_builder)
                .with_auth_http_middleware(ssz_layer),
        )
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
        target_gas_limit: None,
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
            "fork_scoped_endpoints": ["payloads", "payloads/witness", "forkchoice", "bodies"],
            "independently_versioned": {
                "blobs": ["v1", "v2", "v3", "v4"],
            },
            "unscoped_endpoints": ["capabilities", "identity"],
            "limits": {
                "bodies.max_count": 128,
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

    let blobs_response = client
        .post(format!("{auth_url}{ENGINE_V1_BLOBS_ROUTE}"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .body(versioned_hashes.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(blobs_response.status(), reqwest::StatusCode::OK);

    let blobs =
        Vec::<Option<BlobAndProofV1>>::from_ssz_bytes(&blobs_response.bytes().await?).unwrap();
    assert_eq!(blobs.len(), versioned_hashes.len());
    assert!(blobs.iter().all(Option::is_some));

    let fcu = SszForkchoiceUpdateResponse::from_ssz_bytes(&fcu_response.bytes().await?).unwrap();
    assert_eq!(fcu.payload_status.status, PayloadStatusEnum::Valid);

    node.wait_block(1, block_hash, false).await?;

    Ok(())
}

#[tokio::test]
async fn test_engine_ssz_proxy_payloads_witness_returns_execution_witness() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .amsterdam_activated()
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

    let (ssz_layer, ssz_handle) = EngineSszProxyLayer::new();
    let engine_api_handle = ssz_handle.clone();
    let engine_api_builder = EngineApiExt::new(
        BasicEngineApiBuilder::<EthereumEngineValidatorBuilder>::default(),
        move |engine_api| {
            engine_api_handle.set_engine_api_sync(engine_api);
        },
    );
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(
            EthereumAddOns::default()
                .with_engine_api(engine_api_builder)
                .with_auth_http_middleware(ssz_layer),
        )
        .launch()
        .await?;

    ssz_handle.set_witness_handler_sync(Arc::new(EngineSszWitnessGenerator::new(
        node.provider.clone(),
        node.evm_config.clone(),
        node.task_executor.clone(),
    )));

    let node = NodeTestContext::new(node, eth_payload_attributes_amsterdam).await?;
    let wallets = Wallet::new(1).wallet_gen();
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallets[0].clone()).await;

    let payload_attributes = PayloadAttributes {
        timestamp: chain_spec.genesis().timestamp + 1,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
        slot_number: Some(chain_spec.genesis().timestamp + 1),
        target_gas_limit: None,
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
    let block_hash = payload.payload_inner.payload_inner.payload_inner.block_hash;
    let request =
        ExecutionPayloadEnvelopeAmsterdam::from((payload, B256::ZERO, envelope.execution_requests));
    let client = reqwest::Client::new();
    let auth_server = node.auth_server_handle();
    let auth_url = auth_server.http_url();
    let auth_header = secret_to_bearer_header(auth_server.jwt_secret());

    let response = client
        .post(format!("{auth_url}{ENGINE_PAYLOADS_WITNESS_ROUTE}"))
        .header(reqwest::header::AUTHORIZATION, auth_header.to_str()?)
        .header(ENGINE_EXECUTION_VERSION_HEADER, ENGINE_AMSTERDAM_FORK_HEADER)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .body(request.as_ssz_bytes())
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let response = PayloadStatusWithWitness::from_ssz_bytes(&response.bytes().await?).unwrap();
    assert_eq!(response.payload_status.status, PayloadStatusEnum::Valid);
    assert!(response.witness.is_some());

    node.wait_block(1, block_hash, false).await?;

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
    let tree_config =
        TreeConfig::default().with_sparse_trie_prune_depth(2).with_sparse_trie_max_hot_slots(100);

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
