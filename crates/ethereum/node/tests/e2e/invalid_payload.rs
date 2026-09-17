//! Tests for handling invalid payloads via Engine API.
//!
//! This module tests the scenario where a node receives invalid payloads (e.g., with modified
//! state roots) before receiving valid ones, ensuring the node can recover and continue.

use crate::utils::{eth_payload_attributes, eth_payload_attributes_amsterdam};
use alloy_consensus::proofs::calculate_transaction_root;
use alloy_eips::{eip2718::Decodable2718, eip7685::RequestsOrHash};
use alloy_primitives::{bytes, keccak256, Bytes, B256};
use alloy_rpc_types_engine::{ExecutionPayloadV1, ExecutionPayloadV3, PayloadStatusEnum};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, EthereumHardfork, MAINNET};
use reth_e2e_test_utils::{
    eth_payload_attributes_for_fork, setup_engine, transaction::TransactionTestContext,
};
use reth_ethereum_primitives::TransactionSigned;
use reth_node_ethereum::EthereumNode;
use reth_primitives_traits::SignedTransaction;

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

/// Tests that `engine_newPayloadV1` returns `INVALID` for an in-range but unrecoverable
/// transaction signature, and still accepts the valid payload afterwards.
#[tokio::test]
async fn unrecoverable_signature_is_invalid_payload() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .paris_activated()
            .build(),
    );
    let (mut nodes, wallet) =
        setup_engine::<EthereumNode>(1, chain_spec, false, Default::default(), |timestamp| {
            eth_payload_attributes_for_fork(EthereumHardfork::Paris, timestamp)
        })
        .await?;
    let mut node = nodes.pop().unwrap();

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    node.rpc.inject_tx(raw_tx).await?;
    let payload = node.new_payload().await?;
    let block = payload.block().clone();
    let valid_payload =
        ExecutionPayloadV1::from_block_unchecked(block.hash(), &block.clone().into_block());

    // r = 5, s = 1, v = 27 are in range, but r = 5 is not a secp256k1 x-coordinate.
    let raw_tx =
        bytes!("e48085174876e8008252089400000000000000000000000000000000000000ee01801b0501");
    let tx = TransactionSigned::decode_2718_exact(&raw_tx)?;
    let recovery_error = tx.try_recover().unwrap_err();

    // Recompute the transaction root and block hash so validation reaches sender recovery.
    let mut invalid_block = block.clone().into_block();
    invalid_block.body.transactions = vec![tx];
    invalid_block.header.transactions_root =
        calculate_transaction_root(&invalid_block.body.transactions);
    let invalid_payload =
        ExecutionPayloadV1::from_block_unchecked(invalid_block.header.hash_slow(), &invalid_block);

    let engine = node.auth_server_handle().http_client();
    let status = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v1(
        &engine,
        invalid_payload,
    )
    .await?;
    let PayloadStatusEnum::Invalid { validation_error } = status.status else {
        panic!("Expected INVALID for an unrecoverable signature, got {status:?}");
    };
    assert!(validation_error.contains(&recovery_error.to_string()), "{validation_error}");
    assert_eq!(status.latest_valid_hash, Some(block.parent_hash));

    let status = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v1(
        &engine,
        valid_payload,
    )
    .await?;
    assert_eq!(status.status, PayloadStatusEnum::Valid);
    assert_eq!(status.latest_valid_hash, Some(block.hash()));

    Ok(())
}

/// Tests that `engine_newPayloadV5` returns `INVALID` with no latest valid hash for undecodable
/// block access list bytes.
#[tokio::test]
async fn undecodable_bal_is_invalid_payload() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .amsterdam_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes_amsterdam,
    )
    .await?;
    let mut node = nodes.pop().unwrap();

    // Build a valid Amsterdam payload without making it canonical.
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    node.rpc.inject_tx(raw_tx).await?;
    let payload = node.new_payload().await?;
    let block = payload.block().clone();
    let envelope = payload.try_into_v6()?;
    let valid_payload = envelope.execution_payload;

    // Corrupt the block access list bytes and recompute the block hash for the corresponding
    // header, so the payload converts cleanly and the undecodable bytes are the only defect.
    let garbage = Bytes::from_static(b"not-rlp");
    let mut header = block.header().clone();
    header.block_access_list_hash = Some(keccak256(&garbage));

    let mut corrupted = valid_payload.clone();
    corrupted.block_access_list = garbage;
    corrupted.payload_inner.payload_inner.payload_inner.block_hash = header.hash_slow();

    let engine = node.auth_server_handle().http_client();
    let parent_beacon_block_root = block.header().parent_beacon_block_root.unwrap();
    let invalid_status = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v5(
        &engine,
        corrupted,
        vec![],
        parent_beacon_block_root,
        RequestsOrHash::Requests(envelope.execution_requests.clone()),
    )
    .await?;
    assert!(matches!(invalid_status.status, PayloadStatusEnum::Invalid { .. }));
    assert_eq!(invalid_status.latest_valid_hash, None);

    // The same block with well-formed block access list bytes is processed normally.
    let status = EngineApiClient::<reth_node_ethereum::EthEngineTypes>::new_payload_v5(
        &engine,
        valid_payload,
        vec![],
        parent_beacon_block_root,
        RequestsOrHash::Requests(envelope.execution_requests),
    )
    .await?;
    assert!(matches!(status.status, PayloadStatusEnum::Valid));

    Ok(())
}
