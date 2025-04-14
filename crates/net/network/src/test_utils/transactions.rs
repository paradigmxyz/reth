//! Test helper impls for transactions

#![allow(dead_code)]

use crate::{
    cache::LruCache,
    transactions::{
        constants::{
            tx_fetcher::DEFAULT_MAX_COUNT_FALLBACK_PEERS,
            tx_manager::DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
        },
        fetcher::{TransactionFetcher, TxFetchMetadata},
        PeerMetadata, TransactionsManager,
    },
    NetworkConfigBuilder, NetworkManager,
};
use alloy_primitives::TxHash;
use reth_eth_wire::EthVersion;
use reth_eth_wire_types::EthNetworkPrimitives;
use reth_network_api::{PeerKind, PeerRequest, PeerRequestSender};
use reth_network_peers::PeerId;
use reth_storage_api::noop::NoopProvider;
use reth_transaction_pool::test_utils::{testing_pool, TestPool};
use secp256k1::SecretKey;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::trace;

/// A new tx manager for testing.
pub async fn new_tx_manager(
) -> (TransactionsManager<TestPool, EthNetworkPrimitives>, NetworkManager<EthNetworkPrimitives>) {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let client = NoopProvider::default();

    let config = NetworkConfigBuilder::new(secret_key)
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

/// Directly buffer hahs into tx fetcher for testing.
pub fn buffer_hash_to_tx_fetcher(
    tx_fetcher: &mut TransactionFetcher,
    hash: TxHash,
    peer_id: PeerId,
    retries: u8,
    tx_encoded_length: Option<usize>,
) {
    match tx_fetcher.hashes_fetch_inflight_and_pending_fetch.get_or_insert(hash, || {
        TxFetchMetadata::new(
            retries,
            LruCache::new(DEFAULT_MAX_COUNT_FALLBACK_PEERS as u32),
            tx_encoded_length,
        )
    }) {
        Some(metadata) => {
            metadata.fallback_peers_mut().insert(peer_id);
        }
        None => {
            trace!(target: "net::tx",
                    peer_id=format!("{peer_id:#}"),
                    %hash,
                    "failed to insert hash from peer in schnellru::LruMap, dropping hash"
            )
        }
    }

    tx_fetcher.hashes_pending_fetch.insert(hash);
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
