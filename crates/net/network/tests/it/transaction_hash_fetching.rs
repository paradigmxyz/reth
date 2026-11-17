//! Tests fetching transactions that peers announce by hash over real sessions.

use crate::utils::{funded_transactions, poll_until};
use alloy_primitives::TxHash;
use reth_network::{
    test_utils::Testnet,
    transactions::{TransactionPropagationMode, TransactionsManagerConfig},
};
use reth_provider::test_utils::MockEthProvider;
use reth_tracing::init_test_tracing;
use reth_transaction_pool::{PoolTransaction, TransactionPool};

/// Waits until the pool contains all the given transactions.
async fn wait_for_transactions(pool: &impl TransactionPool, hashes: &[TxHash]) {
    poll_until("transactions", async || {
        (pool.get_all(hashes.to_vec()).len() == hashes.len()).then_some(())
    })
    .await
}

/// Hash-only propagation, so peers must request every transaction they learn about.
fn hashes_only() -> TransactionsManagerConfig {
    TransactionsManagerConfig {
        propagation_mode: TransactionPropagationMode::Max(0),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn announced_transactions_are_fetched_from_announcing_peer() {
    init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net =
        Testnet::create_with(2, provider.clone()).await.with_eth_pool_config(hashes_only()).spawn();
    net.connect_peers().await;
    let [peer0, peer1] = net.peers_array();

    // more transactions than fit into a single request
    let txs = funded_transactions(&provider, 300);
    let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();

    for outcome in peer0.pool().unwrap().add_external_transactions(txs).await {
        outcome.unwrap();
    }

    wait_for_transactions(peer1.pool().unwrap(), &hashes).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transactions_reach_all_peers_by_broadcast_and_fetching() {
    init_test_tracing();

    // with the default propagation mode only some peers receive the transactions in full, the
    // others are announced the hashes and fetch them
    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(5, provider.clone()).await.with_eth_pool().spawn();
    net.connect_peers().await;

    let txs = funded_transactions(&provider, 100);
    let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();

    for outcome in net.peers()[0].pool().unwrap().add_external_transactions(txs).await {
        outcome.unwrap();
    }

    for peer in &net.peers()[1..] {
        wait_for_transactions(peer.pool().unwrap(), &hashes).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn transactions_announced_by_many_peers_are_fetched() {
    init_test_tracing();

    // every peer announces the same transactions to the last peer, which spreads its requests
    // over them
    let provider = MockEthProvider::default().with_genesis_block();
    let net =
        Testnet::create_with(6, provider.clone()).await.with_eth_pool_config(hashes_only()).spawn();
    let (listening_peer, announcing_peers) = net.peers().split_last().unwrap();

    let txs = funded_transactions(&provider, 400);
    let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();

    for peer in announcing_peers {
        for outcome in peer.pool().unwrap().add_external_transactions(txs.clone()).await {
            outcome.unwrap();
        }
    }

    net.connect_peers().await;

    wait_for_transactions(listening_peer.pool().unwrap(), &hashes).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transaction_hash_fetching() {
    init_test_tracing();

    let mut config = hashes_only();
    config.transaction_fetcher_config.max_inflight_requests = 1;

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(10, provider.clone()).await.with_eth_pool_config(config).spawn();

    // connect all the peers first
    net.connect_peers().await;
    let (listening_peer, announcing_peers) = net.peers().split_last().unwrap();

    // every other peer inserts transactions of its own
    let mut hashes = Vec::new();
    for peer in announcing_peers {
        for tx in funded_transactions(&provider, 10) {
            hashes.push(peer.pool().unwrap().add_external_transaction(tx).await.unwrap().hash);
        }
    }

    wait_for_transactions(listening_peer.pool().unwrap(), &hashes).await;
}
