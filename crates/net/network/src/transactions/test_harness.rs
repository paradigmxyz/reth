//! Shared test harness for transaction-fetching implementations.

use super::{
    NetworkTransactionEvent, TransactionPropagationMode, TransactionsManager,
    TransactionsManagerConfig,
};
use crate::{
    test_utils::transactions::{new_mock_session, new_tx_manager_with_config},
    NetworkManager,
};
use alloy_primitives::map::{B256Set, HashMap};
use reth_eth_wire::{EthNetworkPrimitives, EthVersion, NewPooledTransactionHashes};
use reth_network_api::PeerRequest;
use reth_network_p2p::sync::{NetworkSyncUpdater, SyncState};
use reth_network_peers::PeerId;
use reth_transaction_pool::test_utils::TestPool;
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    task::Poll,
};
use tokio::sync::mpsc;

#[cfg(test)]
const FIRST_PEER: PeerId = PeerId::new([1; 64]);
#[cfg(test)]
const SECOND_PEER: PeerId = PeerId::new([2; 64]);
#[cfg(test)]
const EIP1559_TRANSACTION_TYPE: u8 = 2;
// Both fixed EIP-1559 fixtures below are 116 bytes.
#[cfg(test)]
const TEST_TRANSACTION_ENCODED_SIZE: usize = 116;

/// A manager-level transaction-fetching harness backed by deterministic mock peer sessions.
///
/// Keeping the harness at the manager boundary lets the same workload exercise either fetcher
/// implementation without exposing their internal state.
#[derive(Debug)]
pub struct TransactionFetchHarness {
    manager: TransactionsManager<TestPool, EthNetworkPrimitives>,
    pool: TestPool,
    _network: NetworkManager<EthNetworkPrimitives>,
    requests: HashMap<PeerId, mpsc::Receiver<PeerRequest>>,
}

impl TransactionFetchHarness {
    /// Creates a harness with one ETH/68 mock session for every peer.
    pub async fn new(peers: impl IntoIterator<Item = PeerId>) -> Self {
        let config = TransactionsManagerConfig {
            propagation_mode: TransactionPropagationMode::Max(0),
            ..Default::default()
        };
        let (mut manager, network) = new_tx_manager_with_config(config).await;
        manager.network.update_sync_state(SyncState::Idle);

        let mut requests = HashMap::default();
        for peer_id in peers {
            let (peer, receiver) = new_mock_session(peer_id, EthVersion::Eth68);
            manager.peers.insert(peer_id, peer);
            requests.insert(peer_id, receiver);
        }

        let pool = manager.pool.clone();
        Self { manager, pool, _network: network, requests }
    }

    /// Delivers a pooled-transaction-hash announcement to the manager.
    pub fn announce(&mut self, peer_id: PeerId, msg: NewPooledTransactionHashes) {
        self.manager.on_network_tx_event(
            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg },
        );
    }

    /// Polls the manager once, driving request scheduling and completed responses.
    pub async fn poll_once(&mut self) {
        poll_fn(|cx| {
            let _ = Pin::new(&mut self.manager).poll(cx);
            Poll::Ready(())
        })
        .await;
    }

    /// Takes the next request sent to `peer_id`.
    pub fn take_request(&mut self, peer_id: PeerId) -> PeerRequest {
        self.requests
            .get_mut(&peer_id)
            .expect("peer is registered")
            .try_recv()
            .expect("manager sent a request to the mock peer")
    }

    /// Drains all immediately available pooled-transaction requests.
    pub fn drain_request_stats(&mut self) -> TransactionFetchRequestStats {
        let mut request_count = 0;
        let mut requested_hash_count = 0;
        let mut unique_hashes = B256Set::default();

        for receiver in self.requests.values_mut() {
            while let Ok(request) = receiver.try_recv() {
                if let PeerRequest::GetPooledTransactions { request, .. } = request {
                    request_count += 1;
                    requested_hash_count += request.0.len();
                    unique_hashes.extend(request.0);
                }
            }
        }

        TransactionFetchRequestStats {
            request_count,
            requested_hash_count,
            unique_requested_hash_count: unique_hashes.len(),
        }
    }

    /// Returns the test transaction pool.
    pub const fn pool(&self) -> &TestPool {
        &self.pool
    }
}

/// Observable request totals for a transaction-fetching workload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransactionFetchRequestStats {
    /// Number of `GetPooledTransactions` requests emitted.
    pub request_count: usize,
    /// Total hashes across all emitted requests.
    pub requested_hash_count: usize,
    /// Number of distinct hashes across all emitted requests.
    pub unique_requested_hash_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transactions::PooledTransactions;
    use alloy_primitives::{hex, TxHash};
    use alloy_rlp::Decodable;
    use reth_eth_wire::NewPooledTransactionHashes68;
    use reth_ethereum_primitives::{PooledTransactionVariant, TransactionSigned};
    use reth_network_p2p::error::RequestError;
    use reth_transaction_pool::TransactionPool;

    fn announcement(hashes: &[TxHash]) -> NewPooledTransactionHashes {
        NewPooledTransactionHashes::Eth68(NewPooledTransactionHashes68 {
            types: vec![EIP1559_TRANSACTION_TYPE; hashes.len()],
            sizes: vec![TEST_TRANSACTION_ENCODED_SIZE; hashes.len()],
            hashes: hashes.to_vec(),
        })
    }

    fn transactions() -> Vec<PooledTransactionVariant> {
        let first = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598daa"
        );
        let second = hex!(
            "02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76"
        );
        [first.as_slice(), second.as_slice()]
            .into_iter()
            .map(|raw| TransactionSigned::decode(&mut &raw[..]).unwrap().try_into().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn retries_failed_request_from_alternate_peer() {
        let transactions = transactions();
        let hashes = transactions.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let expected_hashes = hashes.iter().copied().collect::<B256Set>();
        let mut harness = TransactionFetchHarness::new([FIRST_PEER, SECOND_PEER]).await;

        harness.announce(FIRST_PEER, announcement(&hashes));
        harness.poll_once().await;

        let PeerRequest::GetPooledTransactions { request, response } =
            harness.take_request(FIRST_PEER)
        else {
            panic!("unexpected peer request")
        };
        assert_eq!(request.0.iter().copied().collect::<B256Set>(), expected_hashes);

        response.send(Err(RequestError::Timeout)).unwrap();
        harness.poll_once().await;

        harness.announce(SECOND_PEER, announcement(&hashes));
        harness.poll_once().await;

        let PeerRequest::GetPooledTransactions { request, response } =
            harness.take_request(SECOND_PEER)
        else {
            panic!("unexpected peer request")
        };
        assert_eq!(request.0.iter().copied().collect::<B256Set>(), expected_hashes);

        response.send(Ok(PooledTransactions(transactions))).unwrap();
        harness.poll_once().await;

        assert_eq!(harness.pool().get_all(hashes).len(), expected_hashes.len());
    }
}
