//! Tests fetching transactions that peers announce by hash over real sessions.

use alloy_primitives::{TxHash, U256};
use reth_network::{
    test_utils::Testnet,
    transactions::{TransactionPropagationMode, TransactionsManagerConfig},
};
use reth_primitives_traits::SignedTransaction;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_tracing::init_test_tracing;
use reth_transaction_pool::{
    test_utils::TransactionGenerator, EthPooledTransaction, PoolTransaction, TransactionPool,
};
use std::time::Duration;

/// How long a peer may take to fetch and import the announced transactions.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Generates pooled transactions from distinct senders that are funded in the provider.
fn funded_transactions(provider: &MockEthProvider, count: usize) -> Vec<EthPooledTransaction> {
    let mut generator = TransactionGenerator::with_num_signers(rand::rng(), count);
    generator
        .signer_keys
        .clone()
        .into_iter()
        .map(|signer| {
            let mut tx = generator.transaction();
            tx.signer = signer;
            let tx = tx.into_eip1559().try_into_recovered().unwrap();
            let tx = EthPooledTransaction::try_from_consensus(tx).unwrap();
            provider.add_account(tx.sender(), ExtendedAccount::new(0, U256::from(100_000_000)));
            tx
        })
        .collect()
}

/// Waits until the pool contains all the given transactions.
async fn wait_for_transactions(pool: &impl TransactionPool, hashes: &[TxHash]) {
    tokio::time::timeout(FETCH_TIMEOUT, async {
        loop {
            if pool.get_all(hashes.to_vec()).len() == hashes.len() {
                return
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out, pool has {} of {} transactions",
            pool.get_all(hashes.to_vec()).len(),
            hashes.len()
        )
    });
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
    let net = Testnet::create_with(2, provider.clone()).await;
    let net = net.with_eth_pool_config(hashes_only());
    let handle = net.spawn();
    handle.connect_peers().await;

    // more transactions than fit into a single request
    let txs = funded_transactions(&provider, 300);
    let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();

    let peer0_pool = handle.peers()[0].pool().unwrap();
    for outcome in peer0_pool.add_external_transactions(txs).await {
        outcome.unwrap();
    }

    wait_for_transactions(handle.peers()[1].pool().unwrap(), &hashes).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transactions_reach_all_peers_by_broadcast_and_fetching() {
    init_test_tracing();

    // with the default propagation mode only some peers receive the transactions in full, the
    // others are announced the hashes and fetch them
    let provider = MockEthProvider::default().with_genesis_block();
    let num_peers = 5;
    let net = Testnet::create_with(num_peers, provider.clone()).await;
    let net = net.with_eth_pool();
    let handle = net.spawn();
    handle.connect_peers().await;

    let txs = funded_transactions(&provider, 100);
    let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();

    let peer0_pool = handle.peers()[0].pool().unwrap();
    for outcome in peer0_pool.add_external_transactions(txs).await {
        outcome.unwrap();
    }

    for peer in &handle.peers()[1..] {
        wait_for_transactions(peer.pool().unwrap(), &hashes).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn transactions_announced_by_many_peers_are_fetched() {
    init_test_tracing();

    // every peer announces the same transactions to the last peer, which spreads its requests
    // over them
    let provider = MockEthProvider::default().with_genesis_block();
    let num_peers = 6;
    let net = Testnet::create_with(num_peers, provider.clone()).await;
    let net = net.with_eth_pool_config(hashes_only());
    let handle = net.spawn();
    handle.connect_peers().await;

    let txs = funded_transactions(&provider, 400);
    let hashes = txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();

    for peer in &handle.peers()[..num_peers - 1] {
        for outcome in peer.pool().unwrap().add_external_transactions(txs.clone()).await {
            outcome.unwrap();
        }
    }

    let listening_peer = &handle.peers()[num_peers - 1];
    wait_for_transactions(listening_peer.pool().unwrap(), &hashes).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transaction_hash_fetching() {
    init_test_tracing();

    let mut config = hashes_only();
    config.transaction_fetcher_config.max_inflight_requests = 1;

    let provider = MockEthProvider::default().with_genesis_block();
    let num_peers = 10;
    let net = Testnet::create_with(num_peers, provider.clone()).await;

    // install request handlers
    let net = net.with_eth_pool_config(config);
    let handle = net.spawn();

    // connect all the peers first
    handle.connect_peers().await;

    let listening_peer = &handle.peers()[num_peers - 1];
    let mut listening_peer_tx_listener =
        listening_peer.pool().unwrap().pending_transactions_listener();

    let num_tx_per_peer = 10;

    // Generate transactions for peers
    for i in 1..num_peers {
        let peer = &handle.peers()[i];
        let peer_pool = peer.pool().unwrap();

        for _ in 0..num_tx_per_peer {
            let mut tx_gen = TransactionGenerator::new(rand::rng());
            let tx = tx_gen.gen_eip1559_pooled();
            let sender = tx.sender();
            provider.add_account(sender, ExtendedAccount::new(0, U256::from(100_000_000)));
            peer_pool.add_external_transaction(tx).await.unwrap();
        }
    }

    // Total expected transactions
    let total_expected_tx = num_tx_per_peer * (num_peers - 1);
    let mut received_tx = 0;

    loop {
        tokio::select! {
            Some(_) = listening_peer_tx_listener.recv() => {
                received_tx += 1;
                if received_tx >= total_expected_tx {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                panic!("Timed out waiting for transactions. Received {received_tx}/{total_expected_tx}");
            }
        }
    }
}
