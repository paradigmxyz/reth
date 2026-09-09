//! Test harness driving the transaction fetching pipeline of a [`TransactionsManager`] with mock
//! peer sessions.
//!
//! Announcements are fed to the manager directly and every `GetPooledTransactions` request the
//! manager sends ends up in a mock session channel, from where tests answer it.

use super::{
    NetworkTransactionEvent, TransactionPropagationMode, TransactionsManager,
    TransactionsManagerConfig,
};
use crate::{
    test_utils::transactions::{new_mock_session_with_capacity, new_tx_manager_with_config},
    NetworkManager,
};
use alloy_primitives::TxHash;
use reth_eth_wire::{
    EthNetworkPrimitives, EthVersion, GetPooledTransactions, NewPooledTransactionHashes,
    PooledTransactions,
};
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_network_api::PeerRequest;
use reth_network_p2p::{
    error::RequestResult,
    sync::{NetworkSyncUpdater, SyncState},
};
use reth_network_peers::PeerId;
use reth_transaction_pool::test_utils::TestPool;
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Wake, Waker},
};
use tokio::sync::{mpsc, oneshot};

/// Number of requests a mock session buffers before the manager fails to send to it.
const SESSION_CHANNEL_CAPACITY: usize = 16;

/// Drives a [`TransactionsManager`] whose peers are mock sessions.
struct TxFetchHarness {
    manager: TransactionsManager<TestPool, EthNetworkPrimitives>,
    _network: NetworkManager<EthNetworkPrimitives>,
    sessions: Vec<(PeerId, mpsc::Receiver<PeerRequest>)>,
    /// Keeps the manager's detached pending transaction listener open.
    _pending_transactions: mpsc::Sender<TxHash>,
    wake_flag: Arc<WakeFlag>,
    waker: Waker,
}

impl TxFetchHarness {
    /// Creates a manager with a mock session for every peer.
    async fn new(peers: impl IntoIterator<Item = PeerId>) -> Self {
        Self::with_config(TransactionsManagerConfig::default(), peers).await
    }

    /// Creates a manager with the given config and a mock session for every peer.
    async fn with_config(
        config: TransactionsManagerConfig,
        peers: impl IntoIterator<Item = PeerId>,
    ) -> Self {
        let config = TransactionsManagerConfig {
            propagation_mode: TransactionPropagationMode::Max(0),
            ..config
        };
        let (mut manager, network) = new_tx_manager_with_config(config).await;
        manager.network.update_sync_state(SyncState::Idle);

        // Propagating imported transactions is not part of fetching them and the mock pool can't
        // serve them for propagation, so the manager never learns about pending transactions.
        let (pending_transactions, pending_rx) = mpsc::channel(1);
        manager.pending_transactions = pending_rx;

        let sessions = peers
            .into_iter()
            .map(|peer_id| {
                let (peer, rx) = new_mock_session_with_capacity(
                    peer_id,
                    EthVersion::Eth68,
                    SESSION_CHANNEL_CAPACITY,
                );
                manager.peers.insert(peer_id, peer);
                (peer_id, rx)
            })
            .collect();

        let wake_flag = Arc::new(WakeFlag::default());
        let waker = Waker::from(wake_flag.clone());

        Self {
            manager,
            _network: network,
            sessions,
            _pending_transactions: pending_transactions,
            wake_flag,
            waker,
        }
    }

    /// Delivers an announcement from the peer to the manager.
    fn announce(&mut self, peer_id: PeerId, msg: NewPooledTransactionHashes) {
        self.manager.on_network_tx_event(
            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg },
        );
    }

    /// Polls the manager until a poll completes without asking to be woken again, i.e. until all
    /// buffered work is processed.
    ///
    /// Returns the number of polls.
    fn poll_until_idle(&mut self) -> usize {
        // Polled outside of tokio's cooperative budget: inside a tokio task the manager's channels
        // stop making progress after a number of polls and defer a wake that this loop would
        // never see, leaving responses unprocessed.
        let manager = &mut self.manager;
        let mut manager =
            tokio::task::unconstrained(poll_fn(|cx| Pin::new(&mut *manager).poll(cx)));
        let mut polls = 0;
        loop {
            self.wake_flag.0.store(false, Ordering::Relaxed);
            let mut cx = Context::from_waker(&self.waker);
            let _ = Pin::new(&mut manager).poll(&mut cx);
            polls += 1;
            if !self.wake_flag.0.load(Ordering::Relaxed) {
                return polls
            }
        }
    }

    /// Returns `true` if the manager asked to be polled again since the last poll, e.g. because
    /// a response arrived.
    fn was_woken(&self) -> bool {
        self.wake_flag.0.load(Ordering::Relaxed)
    }

    /// Returns the number of hashes the transaction fetcher is tracking.
    fn num_tracked_hashes(&self) -> usize {
        self.manager.transaction_fetcher.num_hashes()
    }

    /// Takes all `GetPooledTransactions` requests that are queued for the mock sessions.
    fn take_requests(&mut self) -> Vec<MockRequest> {
        let mut requests = Vec::new();
        for (peer_id, rx) in &mut self.sessions {
            while let Ok(request) = rx.try_recv() {
                if let PeerRequest::GetPooledTransactions { request, response } = request {
                    requests.push(MockRequest { peer_id: *peer_id, request, response });
                }
            }
        }
        requests
    }

    /// Returns the manager's transaction pool.
    const fn pool(&self) -> &TestPool {
        &self.manager.pool
    }
}

/// A `GetPooledTransactions` request the manager sent to a mock session.
#[derive(Debug)]
struct MockRequest {
    /// The peer the request was sent to.
    peer_id: PeerId,
    /// The requested hashes.
    request: GetPooledTransactions,
    /// Sends the response to the manager.
    response: oneshot::Sender<RequestResult<PooledTransactions<PooledTransactionVariant>>>,
}

/// Records whether the manager asked to be polled again.
#[derive(Debug, Default)]
struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::{B256Map, B256Set};
    use futures::StreamExt;
    use reth_eth_wire::NewPooledTransactionHashes68;
    use reth_network_p2p::error::RequestError;
    use reth_transaction_pool::{test_utils::TransactionGenerator, TransactionPool};

    const PEER_A: PeerId = PeerId::new([1; 64]);
    const PEER_B: PeerId = PeerId::new([2; 64]);

    fn peer(n: u8) -> PeerId {
        PeerId::new([n; 64])
    }

    fn hash(n: u64) -> TxHash {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&n.to_be_bytes());
        TxHash::from(bytes)
    }

    /// Signed transactions from distinct senders that can be imported into the pool.
    fn pooled_txs(count: usize) -> Vec<PooledTransactionVariant> {
        let mut generator = TransactionGenerator::with_num_signers(rand::rng(), count);
        generator
            .signer_keys
            .clone()
            .into_iter()
            .map(|signer| {
                let mut tx = generator.transaction();
                tx.signer = signer;
                PooledTransactionVariant::try_from(tx.into_eip1559()).unwrap()
            })
            .collect()
    }

    fn announcement(hashes: &[TxHash]) -> NewPooledTransactionHashes {
        NewPooledTransactionHashes::Eth68(NewPooledTransactionHashes68 {
            types: vec![2; hashes.len()],
            sizes: vec![512; hashes.len()],
            hashes: hashes.to_vec(),
        })
    }

    #[tokio::test]
    async fn failed_request_is_retried_from_alternate_peer() {
        let txs = pooled_txs(3);
        let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let mut harness = TxFetchHarness::new([PEER_A, PEER_B]).await;

        harness.announce(PEER_A, announcement(&hashes));
        harness.announce(PEER_B, announcement(&hashes));
        harness.poll_until_idle();

        let mut requests = harness.take_requests();
        assert_eq!(requests.len(), 1, "a hash is only requested from one peer at a time");
        let request = requests.pop().unwrap();
        assert_eq!(request.peer_id, PEER_A);
        let expected = hashes.iter().copied().collect::<B256Set>();
        assert_eq!(request.request.0.iter().copied().collect::<B256Set>(), expected);

        request.response.send(Err(RequestError::Timeout)).unwrap();
        harness.poll_until_idle();

        let mut requests = harness.take_requests();
        assert_eq!(requests.len(), 1);
        let request = requests.pop().unwrap();
        assert_eq!(request.peer_id, PEER_B, "the alternate peer is asked after the failure");
        assert_eq!(request.request.0.iter().copied().collect::<B256Set>(), expected);

        assert!(!harness.was_woken(), "nothing happened since the last poll");
        request.response.send(Ok(PooledTransactions(txs))).unwrap();
        assert!(harness.was_woken(), "the response must wake the manager");
        harness.poll_until_idle();
        assert!(harness.take_requests().is_empty());
        assert_eq!(harness.pool().get_all(hashes).len(), 3, "delivered transactions are imported");
    }

    #[tokio::test]
    async fn responses_are_processed_across_polls() {
        // one request per peer, more than the manager processes per poll iteration
        let peers = (1..=64).map(peer).collect::<Vec<_>>();
        let mut harness = TxFetchHarness::new(peers.iter().copied()).await;
        for (i, peer_id) in peers.iter().enumerate() {
            let hashes = (i as u64 * 64..(i as u64 + 1) * 64).map(hash).collect::<Vec<_>>();
            harness.announce(*peer_id, announcement(&hashes));
        }
        harness.poll_until_idle();
        let requests = harness.take_requests();
        assert_eq!(requests.len(), 64);
        assert_eq!(harness.num_tracked_hashes(), 64 * 64);

        for request in requests {
            request.response.send(Err(RequestError::Timeout)).unwrap();
        }
        let polls = harness.poll_until_idle();
        assert!(
            polls > 1,
            "the fetch events exceed the budget of a single poll, got {polls} polls"
        );
        // Each sole source gets one timeout retry, still processed across multiple polls.
        let retries = harness.take_requests();
        assert_eq!(retries.len(), 64);
        for request in retries {
            request.response.send(Err(RequestError::Timeout)).unwrap();
        }
        harness.poll_until_idle();
        assert_eq!(harness.num_tracked_hashes(), 0);
        assert!(harness.take_requests().is_empty());
    }

    #[tokio::test]
    async fn responses_are_processed_beyond_the_cooperative_budget() {
        // tokio's channels stop making progress after 128 successful polls within one task poll,
        // the harness must not be fooled by that into reporting an idle manager
        let txs = pooled_txs(200);
        let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let by_hash = txs.iter().map(|tx| (*tx.tx_hash(), tx.clone())).collect::<B256Map<_>>();
        let peers = (1..=200).map(peer).collect::<Vec<_>>();
        let mut harness = TxFetchHarness::new(peers.iter().copied()).await;

        // every peer announces one hash, so every hash needs its own response
        for (peer_id, hash) in peers.iter().zip(&hashes) {
            harness.announce(*peer_id, announcement(std::slice::from_ref(hash)));
        }
        harness.poll_until_idle();
        let mut responses = 0;
        loop {
            let requests = harness.take_requests();
            if requests.is_empty() {
                break
            }
            for request in requests {
                let txs = request.request.0.iter().map(|hash| by_hash[hash].clone()).collect();
                request.response.send(Ok(PooledTransactions(txs))).unwrap();
                responses += 1;
                harness.poll_until_idle();
            }
        }

        assert_eq!(responses, 200);
        assert_eq!(harness.num_tracked_hashes(), 0);
        assert_eq!(harness.pool().get_all(hashes).len(), 200, "all transactions are imported");
    }

    #[tokio::test]
    async fn concurrent_responses_wait_for_pool_import_capacity() {
        let txs = pooled_txs(2);
        let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let config =
            TransactionsManagerConfig { max_pending_pool_imports: 1, ..Default::default() };
        let mut harness = TxFetchHarness::with_config(config, [PEER_A, PEER_B]).await;
        harness.announce(PEER_A, announcement(&hashes[..1]));
        harness.announce(PEER_B, announcement(&hashes[1..]));
        harness.poll_until_idle();
        let requests = harness.take_requests();
        assert_eq!(requests.len(), 2);
        for request in requests {
            let tx = txs.iter().find(|tx| request.request.0.contains(tx.tx_hash())).unwrap();
            request.response.send(Ok(PooledTransactions(vec![tx.clone()]))).unwrap();
        }

        // Both responses resolve before either import future is polled.
        for _ in 0..2 {
            let event = harness.manager.transaction_fetcher.next().await.unwrap();
            harness.manager.on_fetch_event(event);
            assert_eq!(
                harness
                    .manager
                    .pending_pool_imports_info
                    .pending_pool_imports
                    .load(Ordering::Relaxed),
                1,
            );
        }
        assert!(harness.manager.pending_fetch_response.is_some());
        harness.poll_until_idle();
        assert!(harness.manager.pending_fetch_response.is_none());
        assert_eq!(harness.pool().get_all(hashes).len(), 2);
    }

    #[tokio::test]
    async fn fetched_response_is_admitted_after_broadcasts_in_bounded_chunks() {
        for broadcast_count in [1, 2] {
            let txs = pooled_txs(2 + broadcast_count);
            let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
            let config =
                TransactionsManagerConfig { max_pending_pool_imports: 2, ..Default::default() };
            let mut harness = TxFetchHarness::with_config(config, [PEER_A, PEER_B]).await;
            harness.announce(PEER_A, announcement(&hashes[..2]));
            harness.poll_until_idle();
            let mut requests = harness.take_requests();
            assert_eq!(requests.len(), 1);

            requests
                .pop()
                .unwrap()
                .response
                .send(Ok(PooledTransactions(txs[..2].to_vec())))
                .unwrap();
            let event = harness.manager.transaction_fetcher.next().await.unwrap();
            // Competing imports consume capacity before the fetched batch is admitted.
            harness.manager.import_transactions(
                PEER_B,
                PooledTransactions(txs[2..].to_vec()),
                crate::transactions::TransactionSource::Broadcast,
            );
            harness.manager.on_fetch_event(event);
            assert_eq!(
                harness
                    .manager
                    .pending_pool_imports_info
                    .pending_pool_imports
                    .load(Ordering::Relaxed),
                2,
            );
            assert_eq!(
                harness.manager.pending_fetch_response.as_ref().unwrap().transactions.0.len(),
                broadcast_count,
            );

            // A disconnect must not discard the buffered bodies, and re-announcements while
            // buffered must not result in another download when dispatch resumes.
            harness.manager.peers.remove(&PEER_A);
            harness.manager.transaction_fetcher.on_peer_disconnected(&PEER_A);
            harness.announce(PEER_B, announcement(&hashes[..2]));
            harness.poll_until_idle();
            assert!(harness.take_requests().is_empty());
            assert!(harness.manager.pending_fetch_response.is_none());
            assert_eq!(harness.pool().get_all(hashes).len(), txs.len());
        }
    }

    #[tokio::test]
    async fn stalled_fetches_cannot_starve_broadcasts() {
        let txs = pooled_txs(1);
        let tx_hash = *txs[0].tx_hash();
        let config =
            TransactionsManagerConfig { max_pending_pool_imports: 2, ..Default::default() };
        let mut harness = TxFetchHarness::with_config(config, [PEER_A, PEER_B]).await;
        harness.announce(PEER_A, announcement(&[hash(0), hash(1)]));
        harness.poll_until_idle();
        let requests = harness.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(harness.manager.transaction_fetcher.num_fetching_hashes(), 2);
        harness.manager.import_transactions(
            PEER_B,
            PooledTransactions(txs),
            crate::transactions::TransactionSource::Broadcast,
        );
        harness.poll_until_idle();
        assert!(
            harness.pool().get(&tx_hash).is_some(),
            "stalled requests must leave broadcast capacity"
        );
        drop(requests);
    }

    #[tokio::test]
    async fn broadcasts_cannot_starve_announced_transactions() {
        let txs = pooled_txs(3);
        let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let config =
            TransactionsManagerConfig { max_pending_pool_imports: 2, ..Default::default() };
        let mut harness = TxFetchHarness::with_config(config, [PEER_A, PEER_B]).await;
        harness.announce(PEER_A, announcement(&hashes[..1]));
        for _ in 0..10 {
            harness.manager.import_transactions(
                PEER_B,
                PooledTransactions(txs[1..].to_vec()),
                crate::transactions::TransactionSource::Broadcast,
            );
            assert!(harness.manager.remaining_pool_import_capacity() >= 1);
        }
        harness.poll_until_idle();
        let request =
            harness.take_requests().pop().expect("broadcast pressure leaves fetch capacity");
        request.response.send(Ok(PooledTransactions(txs[..1].to_vec()))).unwrap();
        // The response retains its original session metadata even if disconnect is processed first.
        harness.manager.peers.remove(&PEER_A);
        harness.manager.transaction_fetcher.on_peer_disconnected(&PEER_A);
        harness.poll_until_idle();
        assert_eq!(harness.pool().get_all(vec![hashes[0]]).len(), 1);
    }
}
