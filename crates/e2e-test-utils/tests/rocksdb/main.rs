//! E2E tests for RocksDB provider functionality.
//!
//! These tests verify that reth can start and operate correctly with RocksDB
//! enabled for various tables instead of the default MDBX storage.

#![cfg(all(feature = "edge", unix))]

use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use alloy_rpc_types_eth::{Transaction, TransactionReceipt};
use eyre::Result;
use jsonrpsee::core::client::ClientT;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{transaction::TransactionTestContext, E2ETestSetupBuilder};
use reth_node_builder::NodeConfig;
use reth_node_core::args::RocksDbArgs;
use reth_node_ethereum::EthereumNode;
use reth_payload_builder::EthPayloadBuilderAttributes;
use std::sync::Arc;

/// Helper function to enable RocksDB for all supported tables.
///
/// This modifies the node configuration to route tx-hash, storages-history,
/// and account-history tables to RocksDB instead of MDBX.
fn with_rocksdb_enabled<C>(mut config: NodeConfig<C>) -> NodeConfig<C> {
    config.rocksdb =
        RocksDbArgs { all: true, tx_hash: true, storages_history: true, account_history: true };
    config
}

/// Test that a node with RocksDB enabled can start up and shut down cleanly.
///
/// This is a minimal smoke test to verify that the RocksDB provider integration
/// works correctly at a basic level.
#[tokio::test]
async fn test_rocksdb_node_startup() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(
                serde_json::from_str(include_str!("../../src/testsuite/assets/genesis.json"))
                    .unwrap(),
            )
            .cancun_activated()
            .build(),
    );

    let attributes_generator = |timestamp| {
        let attributes = alloy_rpc_types_engine::PayloadAttributes {
            timestamp,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
        };
        EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
    };

    let (nodes, _tasks, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, attributes_generator)
            .with_node_config_modifier(with_rocksdb_enabled)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1, "Expected exactly one node to be created");

    let genesis_hash = nodes[0].block_hash(0);
    assert_ne!(genesis_hash, B256::ZERO, "Genesis hash should not be zero");

    Ok(())
}

/// Test that a node with RocksDB enabled can mine blocks and advance the chain.
///
/// This verifies that block production works correctly when using RocksDB
/// for storage, including proper state transitions and block persistence.
#[tokio::test]
async fn test_rocksdb_block_mining() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(
                serde_json::from_str(include_str!("../../src/testsuite/assets/genesis.json"))
                    .unwrap(),
            )
            .cancun_activated()
            .build(),
    );

    let attributes_generator = |timestamp| {
        let attributes = alloy_rpc_types_engine::PayloadAttributes {
            timestamp,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
        };
        EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
    };

    let (mut nodes, _tasks, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, attributes_generator)
            .with_node_config_modifier(with_rocksdb_enabled)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1, "Expected exactly one node to be created");

    let genesis_hash = nodes[0].block_hash(0);
    assert_ne!(genesis_hash, B256::ZERO, "Genesis hash should not be zero");

    // Mine 3 blocks
    for i in 1..=3 {
        let payload = nodes[0].advance_block().await?;
        let block = payload.block();

        assert_eq!(block.number(), i, "Block number should be {i}");
        assert_ne!(block.hash(), B256::ZERO, "Block {i} hash should not be zero");
    }

    // Verify all blocks are stored correctly
    for i in 0..=3 {
        let block_hash = nodes[0].block_hash(i);
        assert_ne!(block_hash, B256::ZERO, "Block {i} should be stored with non-zero hash");
    }

    Ok(())
}

/// Test that a node with RocksDB enabled correctly stores and retrieves transactions.
///
/// This verifies that the TransactionHashNumbers RocksDB table works correctly
/// by injecting transactions, mining blocks, and then querying the transactions
/// via RPC to ensure they can be looked up by hash.
#[tokio::test]
async fn test_rocksdb_transaction_queries() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(
                serde_json::from_str(include_str!("../../src/testsuite/assets/genesis.json"))
                    .unwrap(),
            )
            .cancun_activated()
            .build(),
    );

    let attributes_generator = |timestamp| {
        let attributes = alloy_rpc_types_engine::PayloadAttributes {
            timestamp,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: alloy_primitives::Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
        };
        EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
    };

    let (mut nodes, _tasks, wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec.clone(), attributes_generator)
            .with_node_config_modifier(with_rocksdb_enabled)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1, "Expected exactly one node to be created");

    let chain_id = chain_spec.chain().id();
    let mut tx_hashes = Vec::new();

    // Inject and mine 3 transactions
    for i in 0..3 {
        // Create a new wallet for each tx to avoid nonce issues
        let wallets = wallet.wallet_gen();
        let signer = wallets[0].clone();

        // Create and sign a transfer transaction
        let raw_tx = TransactionTestContext::transfer_tx_bytes(chain_id, signer).await;

        // Inject the transaction into the mempool
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        tx_hashes.push(tx_hash);

        // Mine a block containing the transaction
        let payload = nodes[0].advance_block().await?;
        let block = payload.block();
        assert_eq!(block.number(), i + 1, "Block number should be {}", i + 1);
    }

    // Get the RPC client to query transactions
    let client = nodes[0].rpc_client().expect("RPC client should be available");

    // Query each transaction by hash and verify the responses
    for (i, tx_hash) in tx_hashes.iter().enumerate() {
        let expected_block_number = (i + 1) as u64;

        // Query eth_getTransactionByHash
        let tx: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash]).await?;

        let tx = tx.expect("Transaction should be found by hash");
        assert_eq!(
            tx.block_number,
            Some(expected_block_number),
            "Transaction {} should be in block {}",
            tx_hash,
            expected_block_number
        );

        // Query eth_getTransactionReceipt
        let receipt: Option<TransactionReceipt> =
            client.request("eth_getTransactionReceipt", [tx_hash]).await?;

        let receipt = receipt.expect("Transaction receipt should be found by hash");
        assert_eq!(
            receipt.block_number,
            Some(expected_block_number),
            "Receipt for {} should be in block {}",
            tx_hash,
            expected_block_number
        );
        assert!(receipt.status(), "Transaction {} should have succeeded", tx_hash);
    }

    Ok(())
}
