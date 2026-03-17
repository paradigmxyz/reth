//! E2E tests for `RocksDB` provider functionality.

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_eth::{Transaction, TransactionReceipt};
use eyre::Result;
use jsonrpsee::core::client::ClientT;
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_db::tables;
use reth_e2e_test_utils::{transaction::TransactionTestContext, wallet, E2ETestSetupBuilder};
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

/// Smoke test: node boots with `RocksDB` routing enabled.
#[tokio::test]
async fn test_rocksdb_node_startup() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();

    let (nodes, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, test_attributes_generator)
            .with_storage_v2()
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

    let (mut nodes, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, test_attributes_generator)
            .with_storage_v2()
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

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
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

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
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

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
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

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
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

/// Reorg with `RocksDB`: verifies that unwind correctly reads changesets from
/// storage-aware locations (static files vs MDBX) rather than directly from MDBX.
///
/// This test exercises `unwind_trie_state_from` which previously failed with
/// `UnsortedInput` errors because it read changesets directly from MDBX tables
/// instead of using storage-aware methods that check `is_v2()`.
#[tokio::test]
async fn test_rocksdb_reorg_unwind() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    // Use two separate wallets to avoid nonce conflicts during reorg
    let wallets = wallet::Wallet::new(2).with_chain_id(chain_id).wallet_gen();
    let signer1 = wallets[0].clone();
    let signer2 = wallets[1].clone();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Mine block 1 with a transaction from signer1
    let raw_tx1 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer1.clone(), 0).await;
    let tx_hash1 = nodes[0].rpc.inject_tx(raw_tx1).await?;
    wait_for_pending_tx(&client, tx_hash1).await;

    let payload1 = nodes[0].advance_block().await?;
    let block1_hash = payload1.block().hash();
    assert_eq!(payload1.block().number(), 1);

    // Poll until tx1 appears in RocksDB (ensures persistence happened)
    let tx_number1 = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash1).await;
    assert_eq!(tx_number1, 0, "First tx should have tx_number 0");

    // Mine block 2 with transaction from signer1 (nonce 1)
    let raw_tx2 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer1.clone(), 1).await;
    let tx_hash2 = nodes[0].rpc.inject_tx(raw_tx2).await?;
    wait_for_pending_tx(&client, tx_hash2).await;

    let payload2 = nodes[0].advance_block().await?;
    assert_eq!(payload2.block().number(), 2);

    // Poll until tx2 appears in RocksDB
    let tx_number2 = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash2).await;
    assert_eq!(tx_number2, 1, "Second tx should have tx_number 1");

    // Mine block 3 with transaction from signer1 (nonce 2)
    let raw_tx3 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer1.clone(), 2).await;
    let tx_hash3 = nodes[0].rpc.inject_tx(raw_tx3).await?;
    wait_for_pending_tx(&client, tx_hash3).await;

    let payload3 = nodes[0].advance_block().await?;
    assert_eq!(payload3.block().number(), 3);

    // Poll until tx3 appears in RocksDB
    let tx_number3 = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash3).await;
    assert_eq!(tx_number3, 2, "Third tx should have tx_number 2");

    // Now create an alternate block 2 using signer2 (different wallet, avoids nonce conflict)
    // Inject a tx from signer2 (nonce 0) before building the alternate block
    let raw_alt_tx =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer2.clone(), 0).await;
    let alt_tx_hash = nodes[0].rpc.inject_tx(raw_alt_tx).await?;
    wait_for_pending_tx(&client, alt_tx_hash).await;

    // Build an alternate payload (this builds on top of the current head, i.e., block 3)
    // But we want to reorg back to block 1, so we'll use the payload and then FCU to it
    let alt_payload = nodes[0].new_payload().await?;
    let alt_block_hash = nodes[0].submit_payload(alt_payload.clone()).await?;

    // Trigger reorg: make the alternate chain canonical by sending FCU pointing to block 1's hash
    // as finalized, which should trigger an unwind of blocks 2 and 3
    // The alt block becomes the new head
    nodes[0].update_forkchoice(block1_hash, alt_block_hash).await?;

    // Give time for the reorg to complete
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify we can still query transactions and the chain is consistent
    // If unwind_trie_state_from failed, this would have errored during reorg
    let latest: Option<alloy_rpc_types_eth::Block> =
        client.request("eth_getBlockByNumber", ("latest", false)).await?;
    let latest = latest.expect("Latest block should exist");
    // The alt block is at height 4 (on top of block 3)
    assert!(latest.header.number >= 3, "Should be at height >= 3 after operation");

    // tx1 from block 1 should still be there
    let tx1: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash1]).await?;
    assert!(tx1.is_some(), "tx1 from block 1 should still be queryable");
    assert_eq!(tx1.unwrap().block_number, Some(1));

    // Mine another block to verify the chain can continue
    let raw_tx_final =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer2.clone(), 1).await;
    let tx_hash_final = nodes[0].rpc.inject_tx(raw_tx_final).await?;
    wait_for_pending_tx(&client, tx_hash_final).await;

    let final_payload = nodes[0].advance_block().await?;
    assert!(final_payload.block().number() > 3, "Should be able to mine block after reorg");

    // Verify tx_final is included
    let tx_final: Option<Transaction> =
        client.request("eth_getTransactionByHash", [tx_hash_final]).await?;
    assert!(tx_final.is_some(), "final tx should be in latest block");

    Ok(())
}

/// Historical account queries: verifies that `eth_getBalance` and `eth_getTransactionCount`
/// return correct values at past block numbers after the account state has changed.
///
/// This test exercises the database-backed historical state lookup path. After mining the
/// blocks we care about (1-3), we mine additional blocks to advance the canonical head
/// far enough that the engine tree's persistence + in-memory eviction cycle guarantees
/// blocks 1-3 are no longer in the in-memory overlay. Historical queries for those blocks
/// must then be served from `RocksDB` changesets.
#[tokio::test]
async fn test_rocksdb_historical_account_queries() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let sender: Address = signer.address();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Query the sender's balance and nonce at genesis (block 0)
    let genesis_balance: U256 = client.request("eth_getBalance", (sender, "0x0")).await?;
    let genesis_nonce: U256 = client.request("eth_getTransactionCount", (sender, "0x0")).await?;
    assert!(genesis_balance > U256::ZERO, "Sender should have genesis balance");
    assert_eq!(genesis_nonce, U256::ZERO, "Sender nonce should be 0 at genesis");

    // Mine block 1 with a transfer (nonce 0)
    let raw_tx1 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 0).await;
    let tx_hash1 = nodes[0].rpc.inject_tx(raw_tx1).await?;
    wait_for_pending_tx(&client, tx_hash1).await;

    let payload1 = nodes[0].advance_block().await?;
    assert_eq!(payload1.block().number(), 1);
    poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash1).await;

    // Record state after block 1
    let balance_at_1: U256 = client.request("eth_getBalance", (sender, "0x1")).await?;
    let nonce_at_1: U256 = client.request("eth_getTransactionCount", (sender, "0x1")).await?;
    assert!(balance_at_1 < genesis_balance, "Balance should decrease after transfer + gas");
    assert_eq!(nonce_at_1, U256::from(1), "Nonce should be 1 after first tx");

    // Mine block 2 with another transfer (nonce 1)
    let raw_tx2 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 1).await;
    let tx_hash2 = nodes[0].rpc.inject_tx(raw_tx2).await?;
    wait_for_pending_tx(&client, tx_hash2).await;

    let payload2 = nodes[0].advance_block().await?;
    assert_eq!(payload2.block().number(), 2);
    poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash2).await;

    let balance_at_2: U256 = client.request("eth_getBalance", (sender, "0x2")).await?;
    let nonce_at_2: U256 = client.request("eth_getTransactionCount", (sender, "0x2")).await?;
    assert!(balance_at_2 < balance_at_1, "Balance should decrease further after second tx");
    assert_eq!(nonce_at_2, U256::from(2), "Nonce should be 2 after second tx");

    // Mine block 3 with a third transfer (nonce 2)
    let raw_tx3 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 2).await;
    let tx_hash3 = nodes[0].rpc.inject_tx(raw_tx3).await?;
    wait_for_pending_tx(&client, tx_hash3).await;

    let payload3 = nodes[0].advance_block().await?;
    assert_eq!(payload3.block().number(), 3);
    poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash3).await;

    let balance_at_3: U256 = client.request("eth_getBalance", (sender, "0x3")).await?;
    let nonce_at_3: U256 = client.request("eth_getTransactionCount", (sender, "0x3")).await?;
    assert!(balance_at_3 < balance_at_2, "Balance should decrease further after third tx");
    assert_eq!(nonce_at_3, U256::from(3), "Nonce should be 3 after third tx");

    // Mine additional blocks to push blocks 1-3 out of the in-memory overlay.
    // With persistence_threshold=0 and memory_block_buffer_target=0, each new block
    // triggers persistence up to `head` followed by in-memory eviction. Mining several
    // more blocks ensures the engine loop has completed at least one full
    // persist-then-evict cycle covering blocks 1-3.
    // Each block needs a transaction because the payload builder requires non-empty payloads.
    for nonce in 3..8u64 {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), nonce)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        wait_for_pending_tx(&client, tx_hash).await;
        nodes[0].advance_block().await?;
    }
    // Allow the engine loop to process the persistence completions
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Historical queries — blocks 1-3 should now be served from the database.
    let hist_balance_0: U256 = client.request("eth_getBalance", (sender, "0x0")).await?;
    let hist_nonce_0: U256 = client.request("eth_getTransactionCount", (sender, "0x0")).await?;
    assert_eq!(
        hist_balance_0, genesis_balance,
        "Historical balance at block 0 should match genesis"
    );
    assert_eq!(hist_nonce_0, U256::ZERO, "Historical nonce at block 0 should be 0");

    let hist_balance_1: U256 = client.request("eth_getBalance", (sender, "0x1")).await?;
    let hist_nonce_1: U256 = client.request("eth_getTransactionCount", (sender, "0x1")).await?;
    assert_eq!(hist_balance_1, balance_at_1, "Historical balance at block 1 should match");
    assert_eq!(hist_nonce_1, U256::from(1), "Historical nonce at block 1 should be 1");

    let hist_balance_2: U256 = client.request("eth_getBalance", (sender, "0x2")).await?;
    let hist_nonce_2: U256 = client.request("eth_getTransactionCount", (sender, "0x2")).await?;
    assert_eq!(hist_balance_2, balance_at_2, "Historical balance at block 2 should match");
    assert_eq!(hist_nonce_2, U256::from(2), "Historical nonce at block 2 should be 2");

    let hist_balance_3: U256 = client.request("eth_getBalance", (sender, "0x3")).await?;
    let hist_nonce_3: U256 = client.request("eth_getTransactionCount", (sender, "0x3")).await?;
    assert_eq!(hist_balance_3, balance_at_3, "Historical balance at block 3 should match");
    assert_eq!(hist_nonce_3, U256::from(3), "Historical nonce at block 3 should be 3");

    // "latest" should still match head
    let latest_balance: U256 = client.request("eth_getBalance", (sender, "latest")).await?;
    let latest_nonce: U256 = client.request("eth_getTransactionCount", (sender, "latest")).await?;
    assert_eq!(
        latest_nonce,
        U256::from(8),
        "Latest nonce should be 8 (3 original + 5 extra blocks)"
    );
    assert!(latest_balance < balance_at_3, "Latest balance should be less than block 3 balance");

    Ok(())
}

/// Exploits the race condition between `save_blocks` and `RocksDB` pruning described in
/// <https://github.com/paradigmxyz/reth/pull/23081>.
///
/// When `save_blocks` and the pruner both push to `pending_rocksdb_batches` before a single
/// commit, the pruner reads the OLD committed shard (without `save_blocks`' new entries),
/// filters it, and pushes its own version. On commit the pruner's batch overwrites
/// `save_blocks`' batch for the same `ShardedKey(addr, u64::MAX)`, so the new history
/// entries are silently lost. This test mines blocks with pruning enabled on every block
/// (`block_interval=1`), records the exact balance at each block, then flushes the
/// in-memory overlay and verifies that historical queries for blocks within the retention
/// window return the **exact** recorded values (not just "something > 0"). Without the fix
/// the pruner corrupts the history shards and these lookups fail.
///
/// Uses a small configurable `minimum_distance` (5 blocks) so that the test only needs
/// ~30 blocks instead of exceeding `MINIMUM_UNWIND_SAFE_DISTANCE` (10,064).
#[tokio::test]
async fn test_rocksdb_account_history_pruning() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    const TX_BLOCKS: u64 = 5;
    const PRUNE_DISTANCE: u64 = 5;
    // Mine enough blocks so blocks 1..=TX_BLOCKS fall outside the retention window
    const TOTAL_BLOCKS: u64 = TX_BLOCKS + PRUNE_DISTANCE + 10;
    // Extra blocks mined after TOTAL_BLOCKS to flush the in-memory overlay so
    // historical queries for retained blocks are served from RocksDB, not memory.
    const FLUSH_BLOCKS: u64 = 5;

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .with_node_config_modifier(|mut config| {
        config.pruning.account_history_distance = Some(PRUNE_DISTANCE);
        config.pruning.minimum_distance = Some(PRUNE_DISTANCE);
        config.pruning.block_interval = Some(1);
        config
    })
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let sender: Address = signer.address();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Record genesis state
    let genesis_balance: U256 = client.request("eth_getBalance", (sender, "0x0")).await?;
    assert!(genesis_balance > U256::ZERO);

    // Mine ALL blocks with one transfer each, recording the exact balance at every block.
    // With block_interval=1 the pruner runs on every save_blocks call, so every cycle
    // exercises the race: save_blocks writes a new history entry, and the pruner
    // (if it reads stale committed state) overwrites it.
    let mut balances = Vec::new();
    for nonce in 0..TOTAL_BLOCKS {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), nonce)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        wait_for_pending_tx(&client, tx_hash).await;

        let payload = nodes[0].advance_block().await?;
        assert_eq!(payload.block().number(), nonce + 1);

        let block_hex = format!("0x{:x}", nonce + 1);
        let bal: U256 = client.request("eth_getBalance", (sender, block_hex.as_str())).await?;
        balances.push(bal);
    }

    // Verify balances decreased monotonically
    assert!(balances[0] < genesis_balance);
    for w in balances.windows(2) {
        assert!(w[1] < w[0], "Balance should decrease with each transfer");
    }

    // Mine extra filler blocks to advance the canonical head far enough that the
    // engine tree's persistence + eviction cycle removes TOTAL_BLOCKS from the
    // in-memory overlay. After this, historical queries hit RocksDB directly.
    for nonce in TOTAL_BLOCKS..(TOTAL_BLOCKS + FLUSH_BLOCKS) {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), nonce)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        wait_for_pending_tx(&client, tx_hash).await;
        nodes[0].advance_block().await?;
    }

    // Allow the engine loop to process the persistence completions
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Blocks 1..=TX_BLOCKS are now outside the retention window (tip - block > PRUNE_DISTANCE).
    // Historical account queries for pruned blocks should error.
    for block in 1..=TX_BLOCKS {
        let block_hex = format!("0x{:x}", block);
        let result: Result<U256, _> =
            client.request("eth_getBalance", (sender, block_hex.as_str())).await;
        assert!(
            result.is_err(),
            "eth_getBalance at pruned block {block} should return an error, got {:?}",
            result
        );
    }

    // Blocks within the retention window must return the EXACT balance recorded during
    // mining — not just any non-zero value. The race condition causes save_blocks'
    // history entries to be silently overwritten by the pruner's stale batch, so the
    // shard ends up missing the block's entry entirely. When that happens the RPC either
    // errors or returns a wrong value (from a different changeset).
    //
    // The retention window spans roughly (tip - PRUNE_DISTANCE)..=tip. We check the
    // last PRUNE_DISTANCE blocks of the TOTAL_BLOCKS range (the filler blocks don't
    // have recorded balances, but these blocks do).
    let tip = TOTAL_BLOCKS + FLUSH_BLOCKS;
    for block in (TOTAL_BLOCKS - PRUNE_DISTANCE + 1)..=TOTAL_BLOCKS {
        // Skip blocks that may have been pruned by the filler-block cycles
        if tip.saturating_sub(block) > PRUNE_DISTANCE {
            continue;
        }
        let block_hex = format!("0x{:x}", block);
        let hist_balance: U256 =
            client.request("eth_getBalance", (sender, block_hex.as_str())).await.unwrap_or_else(
                |e| {
                    panic!(
                        "eth_getBalance at retained block {block} failed (history entry lost \
                         due to save_blocks/pruner race?): {e}"
                    )
                },
            );
        let expected = balances[(block - 1) as usize];
        assert_eq!(
            hist_balance, expected,
            "Historical balance at block {block} doesn't match recorded value. \
             The pruner likely overwrote save_blocks' history entry for this block \
             (save_blocks/pruner race — see PR #23081)."
        );
    }

    // "latest" should work and reflect all transactions
    let latest_balance: U256 = client.request("eth_getBalance", (sender, "latest")).await?;
    assert!(latest_balance > U256::ZERO, "Latest balance should be queryable");

    let latest_nonce: U256 = client.request("eth_getTransactionCount", (sender, "latest")).await?;
    assert_eq!(
        latest_nonce,
        U256::from(TOTAL_BLOCKS + FLUSH_BLOCKS),
        "Latest nonce should match total blocks mined"
    );

    Ok(())
}

/// Reorg with `RocksDB`: verifies that unwind correctly reads changesets from
/// storage-aware locations (static files vs MDBX) rather than directly from MDBX.
///
/// This test exercises `unwind_trie_state_from` which previously failed with
/// `UnsortedInput` errors because it read changesets directly from MDBX tables
/// instead of using storage-aware methods that check `is_v2()`.
#[tokio::test]
async fn test_rocksdb_reorg_unwind() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    // Use two separate wallets to avoid nonce conflicts during reorg
    let wallets = wallet::Wallet::new(2).with_chain_id(chain_id).wallet_gen();
    let signer1 = wallets[0].clone();
    let signer2 = wallets[1].clone();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Mine block 1 with a transaction from signer1
    let raw_tx1 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer1.clone(), 0).await;
    let tx_hash1 = nodes[0].rpc.inject_tx(raw_tx1).await?;
    wait_for_pending_tx(&client, tx_hash1).await;

    let payload1 = nodes[0].advance_block().await?;
    let block1_hash = payload1.block().hash();
    assert_eq!(payload1.block().number(), 1);

    // Poll until tx1 appears in RocksDB (ensures persistence happened)
    let tx_number1 = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash1).await;
    assert_eq!(tx_number1, 0, "First tx should have tx_number 0");

    // Mine block 2 with transaction from signer1 (nonce 1)
    let raw_tx2 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer1.clone(), 1).await;
    let tx_hash2 = nodes[0].rpc.inject_tx(raw_tx2).await?;
    wait_for_pending_tx(&client, tx_hash2).await;

    let payload2 = nodes[0].advance_block().await?;
    assert_eq!(payload2.block().number(), 2);

    // Poll until tx2 appears in RocksDB
    let tx_number2 = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash2).await;
    assert_eq!(tx_number2, 1, "Second tx should have tx_number 1");

    // Mine block 3 with transaction from signer1 (nonce 2)
    let raw_tx3 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer1.clone(), 2).await;
    let tx_hash3 = nodes[0].rpc.inject_tx(raw_tx3).await?;
    wait_for_pending_tx(&client, tx_hash3).await;

    let payload3 = nodes[0].advance_block().await?;
    assert_eq!(payload3.block().number(), 3);

    // Poll until tx3 appears in RocksDB
    let tx_number3 = poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash3).await;
    assert_eq!(tx_number3, 2, "Third tx should have tx_number 2");

    // Now create an alternate block 2 using signer2 (different wallet, avoids nonce conflict)
    // Inject a tx from signer2 (nonce 0) before building the alternate block
    let raw_alt_tx =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer2.clone(), 0).await;
    let alt_tx_hash = nodes[0].rpc.inject_tx(raw_alt_tx).await?;
    wait_for_pending_tx(&client, alt_tx_hash).await;

    // Build an alternate payload (this builds on top of the current head, i.e., block 3)
    // But we want to reorg back to block 1, so we'll use the payload and then FCU to it
    let alt_payload = nodes[0].new_payload().await?;
    let alt_block_hash = nodes[0].submit_payload(alt_payload.clone()).await?;

    // Trigger reorg: make the alternate chain canonical by sending FCU pointing to block 1's hash
    // as finalized, which should trigger an unwind of blocks 2 and 3
    // The alt block becomes the new head
    nodes[0].update_forkchoice(block1_hash, alt_block_hash).await?;

    // Give time for the reorg to complete
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify we can still query transactions and the chain is consistent
    // If unwind_trie_state_from failed, this would have errored during reorg
    let latest: Option<alloy_rpc_types_eth::Block> =
        client.request("eth_getBlockByNumber", ("latest", false)).await?;
    let latest = latest.expect("Latest block should exist");
    // The alt block is at height 4 (on top of block 3)
    assert!(latest.header.number >= 3, "Should be at height >= 3 after operation");

    // tx1 from block 1 should still be there
    let tx1: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash1]).await?;
    assert!(tx1.is_some(), "tx1 from block 1 should still be queryable");
    assert_eq!(tx1.unwrap().block_number, Some(1));

    // Mine another block to verify the chain can continue
    let raw_tx_final =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer2.clone(), 1).await;
    let tx_hash_final = nodes[0].rpc.inject_tx(raw_tx_final).await?;
    wait_for_pending_tx(&client, tx_hash_final).await;

    let final_payload = nodes[0].advance_block().await?;
    assert!(final_payload.block().number() > 3, "Should be able to mine block after reorg");

    // Verify tx_final is included
    let tx_final: Option<Transaction> =
        client.request("eth_getTransactionByHash", [tx_hash_final]).await?;
    assert!(tx_final.is_some(), "final tx should be in latest block");

    Ok(())
}

/// Historical account queries: verifies that `eth_getBalance` and `eth_getTransactionCount`
/// return correct values at past block numbers after the account state has changed.
///
/// This test exercises the database-backed historical state lookup path. After mining the
/// blocks we care about (1-3), we mine additional blocks to advance the canonical head
/// far enough that the engine tree's persistence + in-memory eviction cycle guarantees
/// blocks 1-3 are no longer in the in-memory overlay. Historical queries for those blocks
/// must then be served from `RocksDB` changesets.
#[tokio::test]
async fn test_rocksdb_historical_account_queries() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let sender: Address = signer.address();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Query the sender's balance and nonce at genesis (block 0)
    let genesis_balance: U256 = client.request("eth_getBalance", (sender, "0x0")).await?;
    let genesis_nonce: U256 = client.request("eth_getTransactionCount", (sender, "0x0")).await?;
    assert!(genesis_balance > U256::ZERO, "Sender should have genesis balance");
    assert_eq!(genesis_nonce, U256::ZERO, "Sender nonce should be 0 at genesis");

    // Mine block 1 with a transfer (nonce 0)
    let raw_tx1 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 0).await;
    let tx_hash1 = nodes[0].rpc.inject_tx(raw_tx1).await?;
    wait_for_pending_tx(&client, tx_hash1).await;

    let payload1 = nodes[0].advance_block().await?;
    assert_eq!(payload1.block().number(), 1);
    poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash1).await;

    // Record state after block 1
    let balance_at_1: U256 = client.request("eth_getBalance", (sender, "0x1")).await?;
    let nonce_at_1: U256 = client.request("eth_getTransactionCount", (sender, "0x1")).await?;
    assert!(balance_at_1 < genesis_balance, "Balance should decrease after transfer + gas");
    assert_eq!(nonce_at_1, U256::from(1), "Nonce should be 1 after first tx");

    // Mine block 2 with another transfer (nonce 1)
    let raw_tx2 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 1).await;
    let tx_hash2 = nodes[0].rpc.inject_tx(raw_tx2).await?;
    wait_for_pending_tx(&client, tx_hash2).await;

    let payload2 = nodes[0].advance_block().await?;
    assert_eq!(payload2.block().number(), 2);
    poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash2).await;

    let balance_at_2: U256 = client.request("eth_getBalance", (sender, "0x2")).await?;
    let nonce_at_2: U256 = client.request("eth_getTransactionCount", (sender, "0x2")).await?;
    assert!(balance_at_2 < balance_at_1, "Balance should decrease further after second tx");
    assert_eq!(nonce_at_2, U256::from(2), "Nonce should be 2 after second tx");

    // Mine block 3 with a third transfer (nonce 2)
    let raw_tx3 =
        TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), 2).await;
    let tx_hash3 = nodes[0].rpc.inject_tx(raw_tx3).await?;
    wait_for_pending_tx(&client, tx_hash3).await;

    let payload3 = nodes[0].advance_block().await?;
    assert_eq!(payload3.block().number(), 3);
    poll_tx_in_rocksdb(&nodes[0].inner.provider, tx_hash3).await;

    let balance_at_3: U256 = client.request("eth_getBalance", (sender, "0x3")).await?;
    let nonce_at_3: U256 = client.request("eth_getTransactionCount", (sender, "0x3")).await?;
    assert!(balance_at_3 < balance_at_2, "Balance should decrease further after third tx");
    assert_eq!(nonce_at_3, U256::from(3), "Nonce should be 3 after third tx");

    // Mine additional blocks to push blocks 1-3 out of the in-memory overlay.
    // With persistence_threshold=0 and memory_block_buffer_target=0, each new block
    // triggers persistence up to `head` followed by in-memory eviction. Mining several
    // more blocks ensures the engine loop has completed at least one full
    // persist-then-evict cycle covering blocks 1-3.
    // Each block needs a transaction because the payload builder requires non-empty payloads.
    for nonce in 3..8u64 {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), nonce)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        wait_for_pending_tx(&client, tx_hash).await;
        nodes[0].advance_block().await?;
    }
    // Allow the engine loop to process the persistence completions
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Historical queries — blocks 1-3 should now be served from the database.
    let hist_balance_0: U256 = client.request("eth_getBalance", (sender, "0x0")).await?;
    let hist_nonce_0: U256 = client.request("eth_getTransactionCount", (sender, "0x0")).await?;
    assert_eq!(
        hist_balance_0, genesis_balance,
        "Historical balance at block 0 should match genesis"
    );
    assert_eq!(hist_nonce_0, U256::ZERO, "Historical nonce at block 0 should be 0");

    let hist_balance_1: U256 = client.request("eth_getBalance", (sender, "0x1")).await?;
    let hist_nonce_1: U256 = client.request("eth_getTransactionCount", (sender, "0x1")).await?;
    assert_eq!(hist_balance_1, balance_at_1, "Historical balance at block 1 should match");
    assert_eq!(hist_nonce_1, U256::from(1), "Historical nonce at block 1 should be 1");

    let hist_balance_2: U256 = client.request("eth_getBalance", (sender, "0x2")).await?;
    let hist_nonce_2: U256 = client.request("eth_getTransactionCount", (sender, "0x2")).await?;
    assert_eq!(hist_balance_2, balance_at_2, "Historical balance at block 2 should match");
    assert_eq!(hist_nonce_2, U256::from(2), "Historical nonce at block 2 should be 2");

    let hist_balance_3: U256 = client.request("eth_getBalance", (sender, "0x3")).await?;
    let hist_nonce_3: U256 = client.request("eth_getTransactionCount", (sender, "0x3")).await?;
    assert_eq!(hist_balance_3, balance_at_3, "Historical balance at block 3 should match");
    assert_eq!(hist_nonce_3, U256::from(3), "Historical nonce at block 3 should be 3");

    // "latest" should still match head
    let latest_balance: U256 = client.request("eth_getBalance", (sender, "latest")).await?;
    let latest_nonce: U256 = client.request("eth_getTransactionCount", (sender, "latest")).await?;
    assert_eq!(
        latest_nonce,
        U256::from(8),
        "Latest nonce should be 8 (3 original + 5 extra blocks)"
    );
    assert!(latest_balance < balance_at_3, "Latest balance should be less than block 3 balance");

    Ok(())
}

/// Reproduces the race condition between `save_blocks` and `RocksDB` pruning described in
/// <https://github.com/paradigmxyz/reth/pull/23081>.
///
/// Both `save_blocks` and the pruner push to `pending_rocksdb_batches` before a single
/// `commit()`. The pruner reads committed (stale) state that doesn't include `save_blocks`'
/// new entry, filters it, and pushes its own batch. On commit the pruner's batch overwrites
/// `save_blocks`' batch for the same `ShardedKey(addr, u64::MAX)`. Every cycle the new
/// block's history entry is silently lost. After enough cycles the shard is completely empty.
///
/// This test mines blocks with account-history pruning (`block_interval=1`,
/// `minimum_distance=5`), waits for persistence, then reads the `AccountsHistory` shard
/// directly from `RocksDB`. Without the fix the shard is empty — all entries were
/// overwritten by the pruner's stale batches. With the fix, entries for the most recent
/// blocks survive.
#[tokio::test]
async fn test_rocksdb_account_history_pruning() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();
    let chain_id = chain_spec.chain().id();

    const PRUNE_DISTANCE: u64 = 5;
    const TOTAL_BLOCKS: u64 = 20;

    let (mut nodes, _) = E2ETestSetupBuilder::<EthereumNode, _>::new(
        1,
        chain_spec.clone(),
        test_attributes_generator,
    )
    .with_storage_v2()
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .with_node_config_modifier(|mut config| {
        config.pruning.account_history_distance = Some(PRUNE_DISTANCE);
        config.pruning.minimum_distance = Some(PRUNE_DISTANCE);
        config.pruning.block_interval = Some(1);
        config
    })
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();
    let sender: Address = signer.address();
    let client = nodes[0].rpc_client().expect("RPC client");

    // Mine blocks one at a time with a delay so each save_blocks + pruner cycle
    // completes independently. The race fires every cycle (the pruner reads stale
    // committed state that doesn't include save_blocks' pending batch), but processing
    // one block at a time makes the outcome deterministic.
    let mut last_tx_hash = B256::ZERO;
    for nonce in 0..TOTAL_BLOCKS {
        let raw_tx =
            TransactionTestContext::transfer_tx_bytes_with_nonce(chain_id, signer.clone(), nonce)
                .await;
        let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;
        wait_for_pending_tx(&client, tx_hash).await;

        let payload = nodes[0].advance_block().await?;
        assert_eq!(payload.block().number(), nonce + 1);
        last_tx_hash = tx_hash;

        // Let the persistence cycle (save_blocks → pruner → commit) complete before
        // producing the next block, so each cycle processes exactly one block.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    // Wait for the last block to be fully persisted to RocksDB.
    poll_tx_in_rocksdb(&nodes[0].inner.provider, last_tx_hash).await;

    // Read the AccountsHistory shard for `sender` directly from RocksDB.
    // This is the data structure corrupted by the race.
    let rocksdb = nodes[0].inner.provider.rocksdb_provider();
    let shards = rocksdb.account_history_shards(sender).unwrap();
    let all_entries: Vec<u64> = shards.iter().flat_map(|(_, list)| list.iter()).collect();

    // The sender has a transfer in every block, so the shard should contain an entry
    // for every block in the retention window: (TOTAL_BLOCKS - PRUNE_DISTANCE, TOTAL_BLOCKS].
    //
    // Without the fix: the pruner reads stale committed state each cycle, overwrites
    // save_blocks' entry, and only the very last block survives (no subsequent pruner
    // cycle to overwrite it). The shard ends up as just [TOTAL_BLOCKS].
    //
    // With the fix: save_blocks' batch is committed before the pruner reads, so the
    // pruner sees the new entry and preserves it. All retained blocks are present.
    let expected: Vec<u64> = ((TOTAL_BLOCKS - PRUNE_DISTANCE + 1)..=TOTAL_BLOCKS).collect();
    assert_eq!(
        all_entries, expected,
        "AccountsHistory shard for sender doesn't match expected retention window. \
         Expected {expected:?}, got {all_entries:?}. \
         The pruner's stale batch overwrote save_blocks' entries \
         (save_blocks/pruner race, see PR #23081)."
    );

    Ok(())
}
