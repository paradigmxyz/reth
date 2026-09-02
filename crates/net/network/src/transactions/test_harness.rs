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
        let config = TransactionsManagerConfig {
            propagation_mode: TransactionPropagationMode::Max(0),
            ..Default::default()
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
    use alloy_primitives::{map::B256Set, TxHash};
    use reth_eth_wire::NewPooledTransactionHashes68;
    use reth_network_p2p::error::RequestError;
    use reth_transaction_pool::{test_utils::TransactionGenerator, TransactionPool};

    const PEER_A: PeerId = PeerId::new([1; 64]);
    const PEER_B: PeerId = PeerId::new([2; 64]);

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
}
