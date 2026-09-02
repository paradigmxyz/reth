//! Test harness driving the transaction fetching pipeline of a [`TransactionsManager`] with mock
//! peer sessions.
//!
//! Announcements are fed to the manager directly and every `GetPooledTransactions` request the
//! manager sends ends up in a mock session channel, from where tests and benchmarks answer it.

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
    fmt,
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
pub struct TxFetchHarness {
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
    pub async fn new(peers: impl IntoIterator<Item = PeerId>, version: EthVersion) -> Self {
        Self::with_config(TransactionsManagerConfig::default(), peers, version).await
    }

    /// Creates a manager with the given config and a mock session for every peer.
    pub async fn with_config(
        config: TransactionsManagerConfig,
        peers: impl IntoIterator<Item = PeerId>,
        version: EthVersion,
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
                let (peer, rx) =
                    new_mock_session_with_capacity(peer_id, version, SESSION_CHANNEL_CAPACITY);
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
    pub fn announce(&mut self, peer_id: PeerId, msg: NewPooledTransactionHashes) {
        self.manager.on_network_tx_event(
            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg },
        );
    }

    /// Polls the manager until a poll completes without asking to be woken again, i.e. until all
    /// buffered work is processed.
    ///
    /// Returns the number of polls.
    pub fn poll_until_idle(&mut self) -> usize {
        let mut polls = 0;
        loop {
            self.wake_flag.0.store(false, Ordering::Relaxed);
            let mut cx = Context::from_waker(&self.waker);
            let _ = Pin::new(&mut self.manager).poll(&mut cx);
            polls += 1;
            if !self.wake_flag.0.load(Ordering::Relaxed) {
                return polls
            }
        }
    }

    /// Returns `true` if the manager asked to be polled again since the last poll, e.g. because
    /// a response arrived.
    pub fn was_woken(&self) -> bool {
        self.wake_flag.0.load(Ordering::Relaxed)
    }

    /// Returns the number of hashes the transaction fetcher is tracking.
    pub fn num_tracked_hashes(&self) -> usize {
        self.manager.transaction_fetcher.num_hashes()
    }

    /// Takes all `GetPooledTransactions` requests that are queued for the mock sessions.
    pub fn take_requests(&mut self) -> Vec<MockRequest> {
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
    pub const fn pool(&self) -> &TestPool {
        &self.manager.pool
    }
}

impl fmt::Debug for TxFetchHarness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxFetchHarness")
            .field("peers", &self.sessions.iter().map(|(peer_id, _)| peer_id).collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A `GetPooledTransactions` request the manager sent to a mock session.
#[derive(Debug)]
pub struct MockRequest {
    /// The peer the request was sent to.
    pub peer_id: PeerId,
    /// The requested hashes.
    pub request: GetPooledTransactions,
    /// Sends the response to the manager.
    pub response: oneshot::Sender<RequestResult<PooledTransactions<PooledTransactionVariant>>>,
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
    use crate::transactions::constants::tx_fetcher::MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST;
    use alloy_primitives::map::{B256Map, B256Set};
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
        let mut harness = TxFetchHarness::new([PEER_A, PEER_B], EthVersion::Eth68).await;

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

        request.response.send(Ok(PooledTransactions(txs))).unwrap();
        harness.poll_until_idle();
        assert!(harness.take_requests().is_empty());
        assert_eq!(harness.pool().get_all(hashes).len(), 3, "delivered transactions are imported");
    }

    #[tokio::test]
    async fn response_wakes_the_manager() {
        let txs = pooled_txs(2);
        let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let mut harness = TxFetchHarness::new([PEER_A], EthVersion::Eth68).await;

        harness.announce(PEER_A, announcement(&hashes));
        harness.poll_until_idle();
        let mut requests = harness.take_requests();
        assert_eq!(requests.len(), 1);
        assert!(!harness.was_woken(), "nothing happened since the last poll");

        // the inflight request registered its waker while the manager was polled, so the
        // response wakes the manager without any other event
        requests.pop().unwrap().response.send(Ok(PooledTransactions(txs))).unwrap();
        assert!(harness.was_woken(), "the response must wake the manager");

        harness.poll_until_idle();
        assert_eq!(harness.num_tracked_hashes(), 0);
        assert_eq!(harness.pool().get_all(hashes).len(), 2);
    }

    #[tokio::test]
    async fn responses_are_processed_across_polls() {
        // one request per peer, more than the manager processes per poll iteration
        let peers = (1..=64).map(peer).collect::<Vec<_>>();
        let mut harness = TxFetchHarness::new(peers.iter().copied(), EthVersion::Eth68).await;
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
        // no peer is left to fetch the hashes from
        assert_eq!(harness.num_tracked_hashes(), 0);
        assert!(harness.take_requests().is_empty());
    }

    #[tokio::test]
    async fn fetching_is_bounded_by_pool_import_capacity() {
        let txs = pooled_txs(1000);
        let hashes = txs.iter().map(|tx| *tx.tx_hash()).collect::<Vec<_>>();
        let by_hash = txs.iter().map(|tx| (*tx.tx_hash(), tx.clone())).collect::<B256Map<_>>();
        let config =
            TransactionsManagerConfig { max_pending_pool_imports: 300, ..Default::default() };
        let peers = (1..=4).map(peer).collect::<Vec<_>>();
        let mut harness =
            TxFetchHarness::with_config(config, peers.iter().copied(), EthVersion::Eth68).await;

        // every peer announces its own quarter
        for (i, peer_id) in peers.iter().enumerate() {
            harness.announce(*peer_id, announcement(&hashes[i * 250..(i + 1) * 250]));
        }

        let mut total_requests = 0;
        harness.poll_until_idle();
        loop {
            let requests = harness.take_requests();
            if requests.is_empty() {
                break
            }
            // the inflight hashes stay within the import capacity, apart from the minimum
            // request every idle peer is granted
            let inflight = requests.iter().map(|r| r.request.0.len()).sum::<usize>();
            let bound = 300 + peers.len() * MIN_COUNT_HASHES_IN_GET_POOLED_TRANSACTIONS_REQUEST;
            assert!(
                inflight <= bound,
                "requested {inflight} hashes with an import capacity of 300"
            );
            total_requests += requests.len();
            // responses arrive one by one and the pool imports each before the next one arrives
            for request in requests {
                let txs = request.request.0.iter().map(|hash| by_hash[hash].clone()).collect();
                request.response.send(Ok(PooledTransactions(txs))).unwrap();
                harness.poll_until_idle();
            }
        }

        assert!(total_requests >= 4, "the hashes are fetched in several rounds");
        assert_eq!(harness.num_tracked_hashes(), 0);
        assert_eq!(harness.pool().get_all(hashes).len(), 1000, "all transactions are imported");
    }

    #[tokio::test]
    async fn announcement_flood_is_bounded() {
        let mut harness = TxFetchHarness::new([PEER_A], EthVersion::Eth68).await;
        let limit = TransactionsManagerConfig::default()
            .transaction_fetcher_config
            .max_announced_hashes_per_peer as usize;

        // ten full announcements of unique hashes, far more than one peer may have tracked
        for batch in 0..10u64 {
            let hashes = (batch * 4096..(batch + 1) * 4096).map(hash).collect::<Vec<_>>();
            harness.announce(PEER_A, announcement(&hashes));
        }
        assert_eq!(harness.num_tracked_hashes(), limit);

        harness.poll_until_idle();
        let requests = harness.take_requests();
        assert_eq!(requests.len(), 1, "one request at a time per peer");
        assert_eq!(requests[0].request.0.len(), 256);
    }
}
