use crate::utils::{advance_with_random_transactions, eth_payload_attributes};
use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::B256;
use alloy_provider::{Provider, ProviderBuilder};
use futures::future::JoinAll;
use rand::{rngs::StdRng, seq::IndexedRandom, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    setup, setup_engine, setup_engine_with_connection, transaction::TransactionTestContext,
    wallet::Wallet,
};
use reth_node_ethereum::EthereumNode;
use reth_rpc_api::EthApiServer;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, wallet) = setup::<EthereumNode>(
        2,
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

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    // Make the first node advance
    let tx_hash = first_node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let payload = first_node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    first_node.assert_new_block(tx_hash, block_hash, block_number).await?;

    // only send forkchoice update to second node
    second_node.update_forkchoice(block_hash, block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}

#[tokio::test]
async fn e2e_test_send_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new().connect_http(node.rpc_url());

    advance_with_random_transactions(&mut node, 100, &mut rng, true).await?;

    let second_node = nodes.pop().unwrap();
    let second_provider = ProviderBuilder::new().connect_http(second_node.rpc_url());

    assert_eq!(second_provider.get_block_number().await?, 0);

    let head = provider.get_block_by_number(Default::default()).await?.unwrap().header.hash;

    second_node.sync_to(head).await?;

    Ok(())
}

#[tokio::test]
async fn test_long_reorg() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();

    let first_provider = ProviderBuilder::new().connect_http(first_node.rpc_url());

    // Advance first node 100 blocks.
    advance_with_random_transactions(&mut first_node, 100, &mut rng, false).await?;

    // Sync second node to 20th block.
    let head = first_provider.get_block_by_number(20.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    // Produce a fork chain with blocks 21..60
    second_node.payload.timestamp = head.header.timestamp;
    advance_with_random_transactions(&mut second_node, 40, &mut rng, true).await?;

    // Reorg first node from 100th block to new 60th block.
    first_node.sync_to(second_node.block_hash(60)).await?;

    // Advance second node 20 blocks and ensure that first node is able to follow it.
    advance_with_random_transactions(&mut second_node, 20, &mut rng, true).await?;
    first_node.sync_to(second_node.block_hash(80)).await?;

    // Ensure that it works the other way around too.
    advance_with_random_transactions(&mut first_node, 20, &mut rng, true).await?;
    second_node.sync_to(first_node.block_hash(100)).await?;

    Ok(())
}

#[tokio::test]
async fn test_reorg_through_backfill() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();

    let first_provider = ProviderBuilder::new().connect_http(first_node.rpc_url());

    // Advance first node 100 blocks and finalize the chain.
    advance_with_random_transactions(&mut first_node, 100, &mut rng, true).await?;

    // Sync second node to 20th block.
    let head = first_provider.get_block_by_number(20.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    // Produce an unfinalized fork chain with 30 blocks
    second_node.payload.timestamp = head.header.timestamp;
    advance_with_random_transactions(&mut second_node, 30, &mut rng, false).await?;

    // Now reorg second node to the finalized canonical head
    let head = first_provider.get_block_by_number(100.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_propagation() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    // Setup wallet
    let chain_id = chain_spec.chain().into();
    let wallet = Wallet::new(1).inner;
    let mut nonce = 0;
    let mut build_tx = || {
        let mut tx = TxEip1559 {
            chain_id,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 1_000_000_000,
            gas_limit: 100_000,
            nonce,
            ..Default::default()
        };
        nonce += 1;
        let signature = wallet.sign_transaction_sync(&mut tx).unwrap();
        TxEnvelope::Eip1559(tx.into_signed(signature))
    };

    // Setup 10 nodes
    let (mut nodes, _) = setup_engine_with_connection::<EthereumNode>(
        10,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
        false,
    )
    .await?;

    // Connect all nodes to the first one
    let (first, rest) = nodes.split_at_mut(1);
    for node in rest {
        node.connect(&mut first[0]).await;
    }

    // Advance all nodes for 1 block so that they don't consider themselves unsynced
    let tx = build_tx();
    nodes[0].rpc.inject_tx(tx.encoded_2718().into()).await?;
    let payload = nodes[0].advance_block().await?;
    nodes[1..]
        .iter_mut()
        .map(|node| async {
            node.submit_payload(payload.clone()).await.unwrap();
            node.sync_to(payload.block().hash()).await.unwrap();
        })
        .collect::<JoinAll<_>>()
        .await;

    // Build and send transaction to first node
    let tx = build_tx();
    let tx_hash = *tx.tx_hash();
    let _ = nodes[0].rpc.inject_tx(tx.encoded_2718().into()).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Assert that all nodes have the transaction
    for (i, node) in nodes.iter().enumerate() {
        assert!(
            node.rpc.inner.eth_api().transaction_by_hash(tx_hash).await?.is_some(),
            "Node {i} should have the transaction"
        );
    }

    // Build and send one more transaction to a random node
    let tx = build_tx();
    let tx_hash = *tx.tx_hash();
    let _ = nodes.choose(&mut rand::rng()).unwrap().rpc.inject_tx(tx.encoded_2718().into()).await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Assert that all nodes have the transaction
    for node in nodes {
        assert!(node.rpc.inner.eth_api().transaction_by_hash(tx_hash).await?.is_some());
    }

    Ok(())
}

#[tokio::test]
#[ignore = "requires serving in-memory state; serving node keeps ~2 blocks unpersisted"]
async fn can_snap_sync_frozen_head() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    // Do NOT auto-connect nodes — we want to prevent accidental eth sync
    let (mut nodes, _) = setup_engine_with_connection::<EthereumNode>(
        2,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
        false, // do not auto-connect
    )
    .await?;

    let mut node_b = nodes.pop().unwrap();
    let mut node_a = nodes.pop().unwrap();

    // Advance Node A by 300 blocks with random transactions (creates contracts + storage)
    advance_with_random_transactions(&mut node_a, 300, &mut rng, true).await?;

    // Wait for hashed state to stabilize in MDBX
    let _target_root = {
        let mut prev = node_a.snap_state_root().await;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let current = node_a.snap_state_root().await;
            if current == prev && current != B256::ZERO {
                break current;
            }
            prev = current;
        }
    };

    let target_hash = node_a.block_hash(300);

    // Connect Node B to Node A (after blocks are produced, head is frozen)
    node_b.connect(&mut node_a).await;

    // Trigger engine-driven snap sync: send the head FCU so the engine tree
    // detects a fresh node and starts the SnapSyncOrchestrator.
    node_b.sync_to(target_hash).await?;

    // Verify state root matches Node A
    let node_b_root = node_b.snap_state_root().await;
    let node_a_root = node_a.snap_state_root().await;
    assert_eq!(node_b_root, node_a_root, "State roots should match after snap sync");

    Ok(())
}

#[tokio::test]
async fn can_snap_sync_catch_up() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .amsterdam_activated()
            .build(),
    );

    // Do NOT auto-connect — prevent accidental eth sync
    let (mut nodes, _) = setup_engine_with_connection::<EthereumNode>(
        2,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
        false,
    )
    .await?;

    let mut node_b = nodes.pop().unwrap();
    let mut node_a = nodes.pop().unwrap();

    // Build initial state on Node A (20 blocks)
    advance_with_random_transactions(&mut node_a, 20, &mut rng, true).await?;

    let initial_target = node_a.block_hash(20);

    // Connect Node B to Node A
    node_b.connect(&mut node_a).await;

    // Advance Node A further BEFORE triggering snap sync on Node B.
    advance_with_random_transactions(&mut node_a, 10, &mut rng, true).await?;

    // Now trigger snap sync on Node B targeting the initial block.
    node_b.update_forkchoice(initial_target, initial_target).await?;

    // Continue advancing Node A to push even further
    advance_with_random_transactions(&mut node_a, 5, &mut rng, true).await?;

    let final_hash = node_a.block_hash(35);

    // Wait for Node B to sync
    node_b.sync_to(final_hash).await?;

    Ok(())
}

/// Tests that the snap sync orchestrator recovers when the pivot root becomes
/// stale. Node A advances far enough (>128 blocks past the pivot) that the snap
/// server's lookback window no longer covers the original pivot root, forcing the
/// orchestrator to re-resolve the head and advance the pivot.
#[tokio::test]
async fn can_snap_sync_stale_pivot() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .amsterdam_activated()
            .build(),
    );

    let (mut nodes, _) = setup_engine_with_connection::<EthereumNode>(
        2,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
        false,
    )
    .await?;

    let mut node_b = nodes.pop().unwrap();
    let mut node_a = nodes.pop().unwrap();

    // Build 180 blocks so the original pivot root (block 4 = 20 - PIVOT_OFFSET)
    // falls outside ProviderSnapState's 128-block serving lookback window.
    // At tip 180, lookback starts at block 52, so pivot 4 is stale.
    advance_with_random_transactions(&mut node_a, 180, &mut rng, true).await?;
    let old_target = node_a.block_hash(20);

    // Connect Node B to Node A and trigger snap sync targeting block 20.
    // Orchestrator picks pivot = 20 - 16 = 4, whose root is stale.
    node_b.connect(&mut node_a).await;
    node_b.update_forkchoice(old_target, old_target).await?;

    // Advance Node A a bit more while snap sync is running.
    advance_with_random_transactions(&mut node_a, 10, &mut rng, true).await?;

    let final_hash = node_a.block_hash(190);

    // Node B should recover from the stale pivot, re-resolve head, and sync.
    node_b.sync_to(final_hash).await?;

    Ok(())
}
