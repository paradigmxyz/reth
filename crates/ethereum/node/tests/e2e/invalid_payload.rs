//! Tests for handling invalid payloads via Engine API.
//!
//! This module tests the scenario where a node receives invalid payloads (e.g., with modified
//! state roots) before receiving valid ones, ensuring the node can recover and continue.

use crate::utils::eth_payload_attributes;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ExecutionPayloadV3, PayloadStatusEnum};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{setup_engine, transaction::TransactionTestContext};
use reth_node_ethereum::EthereumNode;

use reth_rpc_api::EngineApiClient;
use std::sync::Arc;

/// Tests that a node can handle receiving an invalid payload (with wrong state root)
/// followed by the correct payload, and continue operating normally.
///
/// Setup:
/// - Node 1: Produces valid payloads and advances the chain
/// - Node 2: Receives payloads from node 1, but we also inject modified payloads with invalid state
///   roots in between to verify error handling
#[tokio::test]
async fn can_handle_invalid_payload_then_valid() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut producer = nodes.pop().unwrap();
    let receiver = nodes.pop().unwrap();

    // Get engine API client for the receiver node
    let receiver_engine = receiver.auth_server_handle().http_client();

    // Inject a transaction to allow block building (advance_block waits for transactions)
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    producer.rpc.inject_tx(raw_tx).await?;

    // Build a valid payload on the producer
    let payload = producer.advance_block().await?;
    let valid_block = payload.block().clone();

    // Create valid payload first, then corrupt the state root
    let mut invalid_payload = ExecutionPayloadV3::from_block_unchecked(
        valid_block.hash(),
        &valid_block.clone().into_block(),
    );
    let original_state_root = invalid_payload.payload_inner.payload_inner.state_root;
    invalid_payload.payload_inner.payload_inner.state_root = B256::random_with(&mut rng);

    // Send the invalid payload to the receiver - should be rejected
    let invalid_result = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v3(
        &receiver_engine,
        invalid_payload.clone(),
        vec![],
        valid_block.header().parent_beacon_block_root.unwrap_or_default(),
    )
    .await?;

    println!(
        "Invalid payload response: {:?} (state_root changed from {original_state_root} to {})",
        invalid_result.status, invalid_payload.payload_inner.payload_inner.state_root
    );

    // The invalid payload should be rejected
    assert!(
        matches!(
            invalid_result.status,
            PayloadStatusEnum::Invalid { .. } | PayloadStatusEnum::Syncing
        ),
        "Expected INVALID or SYNCING status for invalid payload, got {:?}",
        invalid_result.status
    );

    // Now send the valid payload - should be accepted
    let valid_payload = ExecutionPayloadV3::from_block_unchecked(
        valid_block.hash(),
        &valid_block.clone().into_block(),
    );

    let valid_result = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v3(
        &receiver_engine,
        valid_payload,
        vec![],
        valid_block.header().parent_beacon_block_root.unwrap_or_default(),
    )
    .await?;

    println!("Valid payload response: {:?}", valid_result.status);

    // The valid payload should be accepted
    assert!(
        matches!(
            valid_result.status,
            PayloadStatusEnum::Valid | PayloadStatusEnum::Syncing | PayloadStatusEnum::Accepted
        ),
        "Expected VALID/SYNCING/ACCEPTED status for valid payload, got {:?}",
        valid_result.status
    );

    // Update forkchoice on receiver to the valid block
    receiver.update_forkchoice(valid_block.hash(), valid_block.hash()).await?;

    // Verify the receiver node is at the expected block
    let receiver_head = receiver.block_hash(1);
    let producer_head = producer.block_hash(1);
    assert_eq!(
        receiver_head, producer_head,
        "Receiver should have synced to the same chain as producer"
    );

    println!(
        "Test passed: Receiver successfully handled invalid payloads and synced to valid chain"
    );

    Ok(())
}

/// Tests that a node can handle multiple consecutive invalid payloads
/// before receiving a valid one.
#[tokio::test]
async fn can_handle_multiple_invalid_payloads() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut producer = nodes.pop().unwrap();
    let receiver = nodes.pop().unwrap();

    let receiver_engine = receiver.auth_server_handle().http_client();

    // Inject a transaction to allow block building
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    producer.rpc.inject_tx(raw_tx).await?;

    // Produce a valid block
    let payload = producer.advance_block().await?;
    let valid_block = payload.block().clone();

    // Send multiple invalid payloads with different corruptions
    for i in 0..3 {
        // Create valid payload first, then corrupt the state root
        let mut invalid_payload = ExecutionPayloadV3::from_block_unchecked(
            valid_block.hash(),
            &valid_block.clone().into_block(),
        );
        invalid_payload.payload_inner.payload_inner.state_root = B256::random_with(&mut rng);

        let result = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v3(
            &receiver_engine,
            invalid_payload,
            vec![],
            valid_block.header().parent_beacon_block_root.unwrap_or_default(),
        )
        .await?;

        println!("Invalid payload {i}: status = {:?}", result.status);

        assert!(
            matches!(result.status, PayloadStatusEnum::Invalid { .. } | PayloadStatusEnum::Syncing),
            "Expected INVALID or SYNCING for invalid payload {i}, got {:?}",
            result.status
        );
    }

    // Now send the valid payload
    let valid_payload = ExecutionPayloadV3::from_block_unchecked(
        valid_block.hash(),
        &valid_block.clone().into_block(),
    );

    let valid_result = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v3(
        &receiver_engine,
        valid_payload,
        vec![],
        valid_block.header().parent_beacon_block_root.unwrap_or_default(),
    )
    .await?;

    println!("Valid payload: status = {:?}", valid_result.status);

    assert!(
        matches!(
            valid_result.status,
            PayloadStatusEnum::Valid | PayloadStatusEnum::Syncing | PayloadStatusEnum::Accepted
        ),
        "Expected valid status for correct payload, got {:?}",
        valid_result.status
    );

    // Finalize the valid block
    receiver.update_forkchoice(valid_block.hash(), valid_block.hash()).await?;

    println!("Test passed: Receiver handled multiple invalid payloads and accepted valid one");

    Ok(())
}

/// Tests invalid payload handling with blocks that contain transactions.
///
/// This test sends real transactions to node 1, produces blocks with those transactions,
/// then sends invalid (corrupted state root) and valid payloads to node 2.
#[tokio::test]
async fn can_handle_invalid_payload_with_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::rng().random();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {seed:?}");

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        2,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;

    let mut producer = nodes.pop().unwrap();
    let receiver = nodes.pop().unwrap();

    let receiver_engine = receiver.auth_server_handle().http_client();

    // Create and send a transaction to the producer node
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let tx_hash = producer.rpc.inject_tx(raw_tx).await?;
    println!("Injected transaction {tx_hash}");

    // Build a block containing the transaction
    let payload = producer.advance_block().await?;
    let valid_block = payload.block().clone();

    // Verify the block contains a transaction
    let tx_count = valid_block.body().transactions().count();
    println!("Block contains {tx_count} transaction(s)");
    assert!(tx_count > 0, "Block should contain at least one transaction");

    // Create invalid payload by corrupting the state root
    let mut invalid_payload = ExecutionPayloadV3::from_block_unchecked(
        valid_block.hash(),
        &valid_block.clone().into_block(),
    );
    let original_state_root = invalid_payload.payload_inner.payload_inner.state_root;
    invalid_payload.payload_inner.payload_inner.state_root = B256::random_with(&mut rng);

    // Send invalid payload - should be rejected
    let invalid_result = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v3(
        &receiver_engine,
        invalid_payload.clone(),
        vec![],
        valid_block.header().parent_beacon_block_root.unwrap_or_default(),
    )
    .await?;

    println!(
        "Invalid payload (with tx) response: {:?} (state_root changed from {original_state_root} to {})",
        invalid_result.status,
        invalid_payload.payload_inner.payload_inner.state_root
    );

    assert!(
        matches!(
            invalid_result.status,
            PayloadStatusEnum::Invalid { .. } | PayloadStatusEnum::Syncing
        ),
        "Expected INVALID or SYNCING for invalid payload with transactions, got {:?}",
        invalid_result.status
    );

    // Send valid payload - should be accepted
    let valid_payload = ExecutionPayloadV3::from_block_unchecked(
        valid_block.hash(),
        &valid_block.clone().into_block(),
    );

    let valid_result = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v3(
        &receiver_engine,
        valid_payload,
        vec![],
        valid_block.header().parent_beacon_block_root.unwrap_or_default(),
    )
    .await?;

    println!("Valid payload (with tx) response: {:?}", valid_result.status);

    assert!(
        matches!(
            valid_result.status,
            PayloadStatusEnum::Valid | PayloadStatusEnum::Syncing | PayloadStatusEnum::Accepted
        ),
        "Expected valid status for correct payload with transactions, got {:?}",
        valid_result.status
    );

    // Update forkchoice
    receiver.update_forkchoice(valid_block.hash(), valid_block.hash()).await?;

    // Verify both nodes are at the same head
    let receiver_head = receiver.block_hash(1);
    let producer_head = producer.block_hash(1);
    assert_eq!(
        receiver_head, producer_head,
        "Receiver should have synced to the same chain as producer"
    );

    println!("Test passed: Receiver handled invalid payloads with transactions correctly");

    Ok(())
}
