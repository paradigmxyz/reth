use crate::utils::{advance_with_random_transactions, eth_payload_attributes};
use alloy_eips::eip7685::RequestsOrHash;
use alloy_genesis::Genesis;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_engine::{PayloadAttributes, PayloadStatusEnum};
use jsonrpsee_core::client::ClientT;
use reth_chainspec::{ChainSpecBuilder, EthChainSpec, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, setup, setup_engine, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_api::TreeConfig;
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_provider::BlockNumReader;
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_tasks::Runtime;
use std::sync::Arc;

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

/// Tests that sparse trie allocation reuse works correctly across consecutive blocks.
///
/// This test exercises the sparse trie allocation reuse path by:
/// 1. Starting a node with parallel state root computation enabled
/// 2. Advancing multiple consecutive blocks with random transactions
/// 3. Verifying that all blocks are successfully validated (state roots match)
///
/// Note: Trie structure reuse is currently disabled due to pruning creating blinded
/// nodes. The preserved trie's allocations are still reused to reduce memory overhead,
/// but the trie is cleared between blocks.
#[tokio::test]
async fn test_sparse_trie_reuse_across_blocks() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Use parallel state root (non-legacy) with pruning enabled
    let tree_config = TreeConfig::default()
        .with_legacy_state_root(false)
        .with_sparse_trie_prune_depth(2)
        .with_sparse_trie_max_storage_tries(100);

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
