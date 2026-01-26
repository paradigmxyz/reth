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
use reth_node_core::args::RocksDbArgs;
use reth_node_ethereum::EthereumNode;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_provider::RocksDBProviderFactory;
use std::sync::Arc;

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

/// Enables `RocksDB` for `TransactionHashNumbers` table.
/// Disables changesets in static files to avoid conflicts with persistence threshold 0.
fn with_rocksdb_enabled<C>(mut config: NodeConfig<C>) -> NodeConfig<C> {
    config.rocksdb = RocksDbArgs { tx_hash: true, ..Default::default() };
    // Disable storage changesets in static files to avoid persistence conflicts
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
            .with_node_config_modifier(with_rocksdb_enabled)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1);

    // Verify RocksDB directory exists
    let rocksdb_path = nodes[0].inner.data_dir.rocksdb();
    assert!(rocksdb_path.exists(), "RocksDB directory should exist at {rocksdb_path:?}");
    assert!(
        std::fs::read_dir(&rocksdb_path).map(|mut d| d.next().is_some()).unwrap_or(false),
        "RocksDB directory should be non-empty"
    );

    let genesis_hash = nodes[0].block_hash(0);
    assert_ne!(genesis_hash, B256::ZERO);

    Ok(())
}

/// Block mining works with `RocksDB` storage.
#[tokio::test]
async fn test_rocksdb_block_mining() -> Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = test_chain_spec();

    let (mut nodes, _tasks, _wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, test_attributes_generator)
            .with_node_config_modifier(with_rocksdb_enabled)
            .build()
            .await?;

    assert_eq!(nodes.len(), 1);

    let genesis_hash = nodes[0].block_hash(0);
    assert_ne!(genesis_hash, B256::ZERO);

    // Mine 3 blocks
    for i in 1..=3 {
        let payload = nodes[0].advance_block().await?;
        let block = payload.block();
        assert_eq!(block.number(), i);
        assert_ne!(block.hash(), B256::ZERO);
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
    .with_node_config_modifier(with_rocksdb_enabled)
    .with_tree_config_modifier(|config| config.with_persistence_threshold(0))
    .build()
    .await?;

    assert_eq!(nodes.len(), 1);

    // Inject and mine a transaction
    let wallets = wallet::Wallet::new(1).with_chain_id(chain_id).wallet_gen();
    let signer = wallets[0].clone();

    let raw_tx = TransactionTestContext::transfer_tx_bytes(chain_id, signer).await;
    let tx_hash = nodes[0].rpc.inject_tx(raw_tx).await?;

    let payload = nodes[0].advance_block().await?;
    assert_eq!(payload.block().number(), 1);

    let tx_hashes = vec![tx_hash];

    let client = nodes[0].rpc_client().expect("RPC client should be available");

    // Query each transaction by hash
    for (i, tx_hash) in tx_hashes.iter().enumerate() {
        let expected_block_number = (i + 1) as u64;

        let tx: Option<Transaction> = client.request("eth_getTransactionByHash", [tx_hash]).await?;
        let tx = tx.expect("Transaction should be found");
        assert_eq!(tx.block_number, Some(expected_block_number));

        let receipt: Option<TransactionReceipt> =
            client.request("eth_getTransactionReceipt", [tx_hash]).await?;
        let receipt = receipt.expect("Receipt should be found");
        assert_eq!(receipt.block_number, Some(expected_block_number));
        assert!(receipt.status());
    }

    let missing_hash = B256::from([0xde; 32]);

    // Direct RocksDB assertions - poll with timeout since persistence is async
    let rocksdb = nodes[0].inner.provider.rocksdb_provider();
    for (i, tx_hash) in tx_hashes.iter().enumerate() {
        let start = std::time::Instant::now();
        loop {
            let tx_number: Option<u64> = rocksdb.get::<tables::TransactionHashNumbers>(*tx_hash)?;
            if let Some(n) = tx_number {
                assert_eq!(n, i as u64, "tx {tx_hash} should have TxNumber {i}");
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "Timed out waiting for tx_hash={tx_hash:?} in RocksDB"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

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
