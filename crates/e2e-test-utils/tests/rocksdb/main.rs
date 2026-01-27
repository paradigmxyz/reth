//! E2E tests for `RocksDB` provider functionality.

#![cfg(all(feature = "edge", unix))]

use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use alloy_rpc_types_eth::{Transaction, TransactionReceipt};
use eyre::Result;
use jsonrpsee::core::client::ClientT;
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_db::tables;
use reth_e2e_test_utils::{transaction::TransactionTestContext, wallet, E2ETestSetupBuilder};
use reth_node_builder::NodeConfig;
use reth_node_ethereum::EthereumNode;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_provider::RocksDBProviderFactory;
use std::{sync::Arc, time::Duration};

const ROCKSDB_POLL_TIMEOUT: Duration = Duration::from_secs(60);
const ROCKSDB_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Polls RPC until the given `tx_hash` is visible as pending (not yet mined).
/// Prevents race conditions where `advance_block` is called before txs are in the pool.
/// Returns the pending transaction.
async fn wait_for_pending_tx<C: ClientT>(client: &C, tx_hash: B256) -> Transaction {
    let start = std::time::Instant::now();
    loop {
        let tx: Option<Transaction> = client
            .request("eth_getTransactionByHash", [tx_hash])
            .await
            .expect("RPC request failed");
        if let Some(tx) = tx {
            assert!(
                tx.block_number.is_none(),
                "Expected pending tx but tx_hash={tx_hash:?} is already mined in block {:?}",
                tx.block_number
            );
            return tx;
        }
        assert!(
            start.elapsed() < ROCKSDB_POLL_TIMEOUT,
            "Timed out after {:?} waiting for tx_hash={tx_hash:?} to appear in pending pool",
            start.elapsed()
        );
        tokio::time::sleep(ROCKSDB_POLL_INTERVAL).await;
    }
}

/// Polls `RocksDB` until the given `tx_hash` appears in `TransactionHashNumbers`.
/// Returns the `tx_number` on success, or panics on timeout.
async fn poll_tx_in_rocksdb<P: RocksDBProviderFactory>(provider: &P, tx_hash: B256) -> u64 {
    let start = std::time::Instant::now();
    let mut interval = ROCKSDB_POLL_INTERVAL;
    loop {
        // Re-acquire handle each iteration to avoid stale snapshot reads
        let rocksdb = provider.rocksdb_provider();
        let tx_number: Option<u64> =
            rocksdb.get::<tables::TransactionHashNumbers>(tx_hash).expect("RocksDB get failed");
        if let Some(n) = tx_number {
            return n;
        }
        assert!(
            start.elapsed() < ROCKSDB_POLL_TIMEOUT,
            "Timed out after {:?} waiting for tx_hash={tx_hash:?} in RocksDB",
            start.elapsed()
        );
        tokio::time::sleep(interval).await;
        // Simple backoff: 50ms -> 100ms -> 200ms (capped)
        interval = std::cmp::min(interval * 2, Duration::from_millis(200));
    }
}

/// Returns the test chain spec for `RocksDB` tests.
fn test_chain_spec() -> Arc<ChainSpec> {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(
                serde_json::from_str(include_str!("../../src/testsuite/assets/genesis.json"))
                    .expect("failed to parse genesis.json"),
            )
            .cancun_activated()
            .build(),
    )
}

/// Returns test payload attributes for the given timestamp.
fn test_attributes_generator(timestamp: u64) -> EthPayloadBuilderAttributes {
    let attributes = alloy_rpc_types_engine::PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: alloy_primitives::Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}

/// Disables static file changesets for test stability.
///
/// Note: Static file changesets are disabled because `persistence_threshold(0)` causes
/// a race where the static file writer expects sequential block numbers but receives
/// them out of order, resulting in `UnexpectedStaticFileBlockNumber` errors.
///
/// RocksDB routing is automatically enabled by default when the `edge` feature is set
/// (see `default_rocksdb_flag()` in `reth-node-core`), so no explicit configuration is needed.
fn without_static_file_changesets<C>(mut config: NodeConfig<C>) -> NodeConfig<C> {
    config.static_files.storage_changesets = false;
    config.static_files.account_changesets = false;
    config
}

/// Smoke test: node boots with `RocksDB` routing enabled.
#[tokio::test]
async fn test_rocksdb_node_startup() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();

    let (nodes, _tasks, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, test_attributes_generator)
            .with_node_config_modifier(without_static_file_changesets)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1);

    // Verify RocksDB provider is functional (can query without error)
    let rocksdb = nodes[0].inner.provider.rocksdb_provider();
    let missing_hash = B256::from([0xab; 32]);
    let result: Option<u64> = rocksdb.get::<tables::TransactionHashNumbers>(missing_hash)?;
    assert!(result.is_none(), "Missing hash should return None");

    let genesis_hash = nodes[0].block_hash(0);
    assert_ne!(genesis_hash, B256::ZERO);

    Ok(())
}

/// Block mining works with `RocksDB` storage.
#[tokio::test]
async fn test_rocksdb_block_mining() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _tasks, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, test_attributes_generator)
            .with_node_config_modifier(without_static_file_changesets)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1);

    let genesis_hash = nodes[0].block_hash(0);
    assert_ne!(genesis_hash, B256::ZERO);

    // Mine 3 blocks with transactions
    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let client = nodes[0].rpc_client().expect("RPC client should be available");

    for i in 1..=3u64 {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), i - 1)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;

        // Wait for tx to enter pending pool before mining
        wait_for_pending_tx(&client, tx_hash).await;

        let payload = nodes[0].advance_block().await?;
        let block = payload.block();
        assert_eq!(block.number(), i);
        assert_ne!(block.hash(), B256::ZERO);

        // Verify tx was actually included in the block
        let receipt: Option<TransactionReceipt> =
            client.request("eth_getTransactionReceipt", [tx_hash]).await?;
        let receipt = receipt.expect("Receipt should exist after mining");
        assert_eq!(receipt.block_number, Some(i), "Tx should be in block {i}");
    }

    // Verify all blocks are stored
    for i in 0..=3 {
        let block_hash = nodes[0].block_hash(i);
        assert_ne!(block_hash, B256::ZERO);
    }

    Ok(())
}

/// Tx hash lookup exercises `TransactionHashNumbers` table.
#[tokio::test]
async fn test_rocksdb_transaction_queries() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _tasks, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_node_config_modifier(without_static_file_changesets)
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    // Inject and mine a transaction
    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let client = nodes[0].rpc_client().expect("RPC client should be available");

    let raw_tx = TransactionTestContext::transfer_tx_bytes(chain_id, signer).await;
    let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;

    // Wait for tx to enter pending pool before mining
    wait_for_pending_tx(&client, tx_hash).await;

    let payload = nodes[0].advance_block().await?;
    assert_eq!(payload.block().number(), 1);

    // Query each transaction by hash
    let tx: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash]).await?;
    let tx = tx.expect("Transaction should be found");
    assert_eq!(tx.block_number, Some(1));

    let receipt: Option<TransactionReceipt> =
        client.request("eth_getTransactionReceipt", [tx_hash]).await?;
    let receipt = receipt.expect("Receipt should be found");
    assert_eq!(receipt.block_number, Some(1));
    assert!(receipt.status());

    // Direct RocksDB assertion - poll with timeout since persistence is async
    let tx_number = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash).await;
    assert_eq!(tx_number, 0, "First tx should have TxNumber 0");

    // Verify missing hash returns None
    let missing_hash = B256::from([0xde; 32]);
    let rocksdb = nodes[0].inner.provider.rocksdb_provider();
    let missing_tx_number: Option<u64> =
        rocksdb.get::<tables::TransactionHashNumbers>(missing_hash)?;
    assert!(missing_tx_number.is_none());

    let missing_tx: Option<Transaction> =
        client.request("eth_getTransactionByHash", [missing_hash]).await?;
    assert!(missing_tx.is_none(), "expected no transaction for missing hash");

    let missing_receipt: Option<TransactionReceipt> =
        client.request("eth_getTransactionReceipt", [missing_hash]).await?;
    assert!(missing_receipt.is_none(), "expected no receipt for missing hash");

    Ok(())
}

/// Multiple transactions in the same block are correctly persisted to `RocksDB`.
#[tokio::test]
async fn test_rocksdb_multi_tx_same_block() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _tasks, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_node_config_modifier(without_static_file_changesets)
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    // Create 3 txs from the same wallet with sequential nonces
    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let client = nodes[0].rpc_client().expect("RPC client");

    let mut tx_hashes = Vec::new();
    for nonce in 0..3 {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), nonce)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        tx_hashes.push(tx_hash);
    }

    // Wait for all txs to appear in pending pool before mining
    for tx_hash in &tx_hashes {
        wait_for_pending_tx(&client, *tx_hash).await;
    }

    // Mine one block containing all 3 txs
    let payload = nodes[0].advance_block().await?;
    assert_eq!(payload.block().number(), 1);

    // Verify block contains all 3 txs
    let block: Option<alloy_rpc_types_eth::Block> =
        client.request("eth_getBlockByNumber", ("0x1", true)).await?;
    let block = block.expect("Block 1 should exist");
    assert_eq!(block.transactions.len(), 3, "Block should contain 3 txs");

    // Verify each tx via RPC
    for tx_hash in &tx_hashes {
        let tx: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash]).await?;
        let tx = tx.expect("Transaction should be found");
        assert_eq!(tx.block_number, Some(1), "All txs should be in block 1");
    }

    // Poll RocksDB for all tx hashes and collect tx_numbers
    let mut tx_numbers = Vec::new();
    for tx_hash in &tx_hashes {
        let n = poll_tx_in_rocksdb(&nodes[0].inner.provider, *tx_hash).await;
        tx_numbers.push(n);
    }

    // Verify tx_numbers form the set {0, 1, 2}
    tx_numbers.sort();
    assert_eq!(tx_numbers, vec![0, 1, 2], "TxNumbers should be 0, 1, 2");

    Ok(())
}

/// Transactions across multiple blocks have globally continuous `tx_numbers`.
#[tokio::test]
async fn test_rocksdb_txs_across_blocks() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _tasks, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_node_config_modifier(without_static_file_changesets)
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Block 1: 2 transactions
    let tx_hash_0 = nodes[0]
        .rpc
        .inject_tx(
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 0).await,
        )
        .await?;
    let tx_hash_1 = nodes[0]
        .rpc
        .inject_tx(
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 1).await,
        )
        .await?;

    // Wait for both txs to appear in pending pool
    wait_for_pending_tx(&client, tx_hash_0).await;
    wait_for_pending_tx(&client, tx_hash_1).await;

    let payload1 = nodes[0].advance_block().await?;
    assert_eq!(payload1.block().number(), 1);

    // Block 2: 1 transaction
    let tx_hash_2 = nodes[0]
        .rpc
        .inject_tx(
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 2).await,
        )
        .await?;

    wait_for_pending_tx(&client, tx_hash_2).await;

    let payload2 = nodes[0].advance_block().await?;
    assert_eq!(payload2.block().number(), 2);

    // Verify block contents via RPC
    let tx0: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash_0]).await?;
    let tx1: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash_1]).await?;
    let tx2: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash_2]).await?;

    assert_eq!(tx0.expect("tx0").block_number, Some(1));
    assert_eq!(tx1.expect("tx1").block_number, Some(1));
    assert_eq!(tx2.expect("tx2").block_number, Some(2));

    // Poll RocksDB and verify global tx_number continuity
    let all_tx_hashes = [tx_hash_0, tx_hash_1, tx_hash_2];
    let mut tx_numbers = Vec::new();
    for tx_hash in &all_tx_hashes {
        let n = poll_tx_in_rocksdb(&nodes[0].inner.provider, *tx_hash).await;
        tx_numbers.push(n);
    }

    // Verify they form a continuous sequence {0, 1, 2}
    tx_numbers.sort();
    assert_eq!(tx_numbers, vec![0, 1, 2], "TxNumbers should be globally continuous: 0, 1, 2");

    // Re-query block 1 txs after block 2 is mined (regression guard)
    let tx0_again: Option<Transaction> =
        client.request("eth_getTransactionByHash", [tx_hash_0]).await?;
    assert!(tx0_again.is_some(), "Block 1 tx should still be queryable after block 2");

    Ok(())
}

/// Pending transactions should NOT appear in `RocksDB` until mined.
#[tokio::test]
async fn test_rocksdb_pending_tx_not_in_storage() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _tasks, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_node_config_modifier(without_static_file_changesets)
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();

    // Inject tx but do NOT mine
    let raw_tx = TransactionTestContext::transfer_tx_bytes(chain_id, signer).await;
    let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;

    // Verify tx is in pending pool via RPC
    let client = nodes[0].rpc_client().expect("RPC client");
    wait_for_pending_tx(&client, tx_hash).await;

    let pending_tx: Option<Transaction> =
        client.request("eth_getTransactionByHash", [tx_hash]).await?;
    assert!(pending_tx.is_some(), "Pending tx should be visible via RPC");
    assert!(pending_tx.unwrap().block_number.is_none(), "Pending tx should have no block_number");

    // Assert tx is NOT in RocksDB before mining (single check - tx is confirmed pending)
    let rocksdb = nodes[0].inner.provider.rocksdb_provider();
    let tx_number: Option<u64> = rocksdb.get::<tables::TransactionHashNumbers>(tx_hash)?;
    assert!(
        tx_number.is_none(),
        "Pending tx should NOT be in RocksDB before mining, but found tx_number={:?}",
        tx_number
    );

    // Now mine the block
    let payload = nodes[0].advance_block().await?;
    assert_eq!(payload.block().number(), 1);

    // Poll until tx appears in RocksDB
    let tx_number = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash).await;
    assert_eq!(tx_number, 0, "First tx should have tx_number 0");

    // Verify tx is now mined via RPC
    let mined_tx: Option<Transaction> =
        client.request("eth_getTransactionByHash", [tx_hash]).await?;
    assert_eq!(mined_tx.expect("mined tx").block_number, Some(1));

    Ok(())
}
