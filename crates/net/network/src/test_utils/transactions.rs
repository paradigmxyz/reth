//! Test helper impls for transactions

#![allow(dead_code)]

use crate::transactions::{
    constants::tx_manager::DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
    fetcher::TransactionFetcher, PeerMetadata, TransactionsManager,
};
use alloy_primitives::TxHash;
use reth_eth_wire::EthVersion;
use reth_eth_wire_types::EthNetworkPrimitives;
use reth_network_api::{PeerKind, PeerRequest, PeerRequestSender};
use reth_network_peers::PeerId;
use reth_storage_api::noop::NoopProvider;
use reth_tasks::Runtime;
use reth_transaction_pool::test_utils::{testing_pool, TestPool};
use secp256k1::SecretKey;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{NetworkConfigBuilder, NetworkManager};

/// A new tx manager for testing.
pub async fn new_tx_manager(
) -> (TransactionsManager<TestPool, EthNetworkPrimitives>, NetworkManager<EthNetworkPrimitives>) {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let client = NoopProvider::default();

    let config = NetworkConfigBuilder::new(secret_key, Runtime::test())
        // let OS choose port
        .listener_port(0)
        .disable_discovery()
        .build(client);

    let pool = testing_pool();

    let transactions_manager_config = config.transactions_manager_config.clone();
    let (_network_handle, network, transactions, _) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .transactions(pool.clone(), transactions_manager_config)
        .split_with_handle();

    (transactions, network)
}

/// Directly buffer hash into tx fetcher Stage 1 (announced) for testing.
pub fn buffer_hash_to_tx_fetcher(
    tx_fetcher: &mut TransactionFetcher,
    hash: TxHash,
    peer_id: PeerId,
) {
    tx_fetcher.insert_announced_for_test(hash, peer_id, 0);
}

/// Mock a new session, returns (peer, channel-to-send-get-pooled-tx-response-on).
pub fn new_mock_session(
    peer_id: PeerId,
    version: EthVersion,
) -> (PeerMetadata<EthNetworkPrimitives>, mpsc::Receiver<PeerRequest>) {
    let (to_mock_session_tx, to_mock_session_rx) = mpsc::channel(1);

    (
        PeerMetadata::new(
            PeerRequestSender::new(peer_id, to_mock_session_tx),
            version,
            Arc::from(""),
            DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
            PeerKind::Trusted,
        ),
        to_mock_session_rx,
    )
}
