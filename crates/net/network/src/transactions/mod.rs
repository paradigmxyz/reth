//! Transactions management for the p2p network.
//!
//! `TransactionFetcher` is responsible for rate limiting and retry logic for fetching
//! transactions. Upon receiving an announcement, functionality of the `TransactionFetcher` is
//! used for filtering out hashes 1) for which the tx is already known and 2) unknown but the hash
//! is already seen in a previous announcement. The hashes that remain from an announcement are
//! then packed into a request with respect to the [`EthVersion`] of the announcement. Any hashes
//! that don't fit into the request, are buffered in the `TransactionFetcher`. If on the other
//! hand, space remains, hashes that the peer has previously announced are taken out of buffered
//! hashes to fill the request up. The [`GetPooledTransactions`] request is then sent to the
//! peer's session, this marks the peer as active with respect to
//! `MAX_CONCURRENT_TX_REQUESTS_PER_PEER`.
//!
//! When a peer buffers hashes in the `TransactionsManager::on_new_pooled_transaction_hashes`
//! pipeline, it is stored as fallback peer for those hashes. When [`TransactionsManager`] is
//! polled, it checks if any of fallback peer is idle. If so, it packs a request for that peer,
//! filling it from the buffered hashes. It does so until there are no more idle peers or until
//! the hashes buffer is empty.
//!
//! If a [`GetPooledTransactions`] request resolves with an error, the hashes in the request are
//! buffered with respect to `MAX_REQUEST_RETRIES_PER_TX_HASH`. So is the case if the request
//! resolves with partial success, that is some of the requested hashes are not in the response,
//! these are then buffered.
//!
//! Most healthy peers will send the same hashes in their announcements, as RLPx is a gossip
//! protocol. This means it's unlikely, that a valid hash, will be buffered for very long
//! before it's re-tried. Nonetheless, the capacity of the buffered hashes cache must be large
//! enough to buffer many hashes during network failure, to allow for recovery.

use crate::{
    cache::LruCache,
    manager::NetworkEvent,
    message::{PeerRequest, PeerRequestSender},
    metrics::{TransactionsManagerMetrics, NETWORK_POOL_TRANSACTIONS_SCOPE},
    NetworkEvents, NetworkHandle,
};
use futures::{stream::FuturesUnordered, Future, StreamExt};
use reth_eth_wire::{
    EthVersion, GetPooledTransactions, HandleMempoolData, HandleVersionedMempoolData,
    NewPooledTransactionHashes, NewPooledTransactionHashes66, NewPooledTransactionHashes68,
    PooledTransactions, RequestTxHashes, Transactions,
};
use reth_interfaces::{
    p2p::error::{RequestError, RequestResult},
    sync::SyncStateProvider,
};
use reth_metrics::common::mpsc::UnboundedMeteredReceiver;
use reth_network_api::{Peers, ReputationChangeKind};
use reth_primitives::{
    FromRecoveredPooledTransaction, PeerId, PooledTransactionsElement, TransactionSigned, TxHash,
    B256,
};
use reth_transaction_pool::{
    error::PoolResult, GetPooledTransactionLimit, PoolTransaction, PropagateKind,
    PropagatedTransactions, TransactionPool, ValidPoolTransaction,
};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    num::NonZeroUsize,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, oneshot::error::RecvError};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};
use tracing::{debug, trace};

/// Aggregation on configurable parameters for [`TransactionsManager`].
pub mod config;
/// Default and spec'd bounds.
pub mod constants;
/// Component responsible for fetching transactions from [`NewPooledTransactionHashes`].
pub mod fetcher;
pub mod validation;
pub use config::{TransactionFetcherConfig, TransactionsManagerConfig};

use constants::SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE;
pub(crate) use fetcher::{FetchEvent, TransactionFetcher};
pub use validation::*;

pub use self::constants::{
    tx_fetcher::DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
    SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
};
use self::constants::{tx_manager::*, DEFAULT_SOFT_LIMIT_BYTE_SIZE_TRANSACTIONS_BROADCAST_MESSAGE};

/// The future for importing transactions into the pool.
///
/// Resolves with the result of each transaction import.
pub type PoolImportFuture = Pin<Box<dyn Future<Output = Vec<PoolResult<TxHash>>> + Send + 'static>>;

/// Api to interact with [TransactionsManager] task.
///
/// This can be obtained via [TransactionsManager::handle] and can be used to manually interact with
/// the [TransactionsManager] task once it is spawned.
///
/// For example [TransactionsHandle::get_peer_transaction_hashes] returns the transaction hashes
/// known by a specific peer.
#[derive(Debug, Clone)]
pub struct TransactionsHandle {
    /// Command channel to the [`TransactionsManager`]
    manager_tx: mpsc::UnboundedSender<TransactionsCommand>,
}

/// Implementation of the `TransactionsHandle` API for use in testnet via type
/// [`PeerHandle`](crate::test_utils::PeerHandle).
impl TransactionsHandle {
    fn send(&self, cmd: TransactionsCommand) {
        let _ = self.manager_tx.send(cmd);
    }

    /// Fetch the [`PeerRequestSender`] for the given peer.
    async fn peer_handle(&self, peer_id: PeerId) -> Result<Option<PeerRequestSender>, RecvError> {
        let (tx, rx) = oneshot::channel();
        self.send(TransactionsCommand::GetPeerSender { peer_id, peer_request_sender: tx });
        rx.await
    }

    /// Requests the transactions directly from the given peer.
    ///
    /// Returns `None` if the peer is not connected.
    ///
    /// **Note**: this returns the response from the peer as received.
    pub async fn get_pooled_transactions_from(
        &self,
        peer_id: PeerId,
        hashes: Vec<B256>,
    ) -> Result<Option<Vec<PooledTransactionsElement>>, RequestError> {
        let Some(peer) = self.peer_handle(peer_id).await? else { return Ok(None) };

        let (tx, rx) = oneshot::channel();
        let request = PeerRequest::GetPooledTransactions { request: hashes.into(), response: tx };
        peer.try_send(request).ok();

        rx.await?.map(|res| Some(res.0))
    }

    /// Manually propagate the transaction that belongs to the hash.
    pub fn propagate(&self, hash: TxHash) {
        self.send(TransactionsCommand::PropagateHash(hash))
    }

    /// Manually propagate the transaction hash to a specific peer.
    ///
    /// Note: this only propagates if the pool contains the transaction.
    pub fn propagate_hash_to(&self, hash: TxHash, peer: PeerId) {
        self.propagate_hashes_to(Some(hash), peer)
    }

    /// Manually propagate the transaction hashes to a specific peer.
    ///
    /// Note: this only propagates the transactions that are known to the pool.
    pub fn propagate_hashes_to(&self, hash: impl IntoIterator<Item = TxHash>, peer: PeerId) {
        self.send(TransactionsCommand::PropagateHashesTo(hash.into_iter().collect(), peer))
    }

    /// Request the active peer IDs from the [`TransactionsManager`].
    pub async fn get_active_peers(&self) -> Result<HashSet<PeerId>, RecvError> {
        let (tx, rx) = oneshot::channel();
        self.send(TransactionsCommand::GetActivePeers(tx));
        rx.await
    }

    /// Manually propagate full transactions to a specific peer.
    pub fn propagate_transactions_to(&self, transactions: Vec<TxHash>, peer: PeerId) {
        self.send(TransactionsCommand::PropagateTransactionsTo(transactions, peer))
    }

    /// Request the transaction hashes known by specific peers.
    pub async fn get_transaction_hashes(
        &self,
        peers: Vec<PeerId>,
    ) -> Result<HashMap<PeerId, HashSet<TxHash>>, RecvError> {
        let (tx, rx) = oneshot::channel();
        self.send(TransactionsCommand::GetTransactionHashes { peers, tx });
        rx.await
    }

    /// Request the transaction hashes known by a specific peer.
    pub async fn get_peer_transaction_hashes(
        &self,
        peer: PeerId,
    ) -> Result<HashSet<TxHash>, RecvError> {
        let res = self.get_transaction_hashes(vec![peer]).await?;
        Ok(res.into_values().next().unwrap_or_default())
    }
}

/// Manages transactions on top of the p2p network.
///
/// This can be spawned to another task and is supposed to be run as background service.
/// [`TransactionsHandle`] can be used as frontend to programmatically send commands to it and
/// interact with it.
///
/// The [`TransactionsManager`] is responsible for:
///    - handling incoming eth messages for transactions.
///    - serving transaction requests.
///    - propagate transactions
///
/// This type communicates with the [`NetworkManager`](crate::NetworkManager) in both directions.
///   - receives incoming network messages.
///   - sends messages to dispatch (responses, propagate tx)
///
/// It is directly connected to the [`TransactionPool`] to retrieve requested transactions and
/// propagate new transactions over the network.
#[derive(Debug)]
#[must_use = "Manager does nothing unless polled."]
pub struct TransactionsManager<Pool> {
    /// Access to the transaction pool.
    pool: Pool,
    /// Network access.
    network: NetworkHandle,
    /// Subscriptions to all network related events.
    ///
    /// From which we get all new incoming transaction related messages.
    network_events: UnboundedReceiverStream<NetworkEvent>,
    /// Transaction fetcher to handle inflight and missing transaction requests.
    transaction_fetcher: TransactionFetcher,
    /// All currently pending transactions grouped by peers.
    ///
    /// This way we can track incoming transactions and prevent multiple pool imports for the same
    /// transaction
    transactions_by_peers: HashMap<TxHash, Vec<PeerId>>,
    /// Transactions that are currently imported into the `Pool`.
    ///
    /// The import process includes:
    ///  - validation of the transactions, e.g. transaction is well formed: valid tx type, fees are
    ///    valid, or for 4844 transaction the blobs are valid. See also
    ///    [EthTransactionValidator](reth_transaction_pool::validate::EthTransactionValidator)
    /// - if the transaction is valid, it is added into the pool.
    ///
    /// Once the new transaction reaches the __pending__ state it will be emitted by the pool via
    /// [TransactionPool::pending_transactions_listener] and arrive at the `pending_transactions`
    /// receiver.
    pool_imports: FuturesUnordered<PoolImportFuture>,
    /// Stats on pending pool imports that help the node self-monitor.
    pending_pool_imports_info: PendingPoolImportsInfo,
    /// Bad imports.
    bad_imports: LruCache<TxHash>,
    /// All the connected peers.
    peers: HashMap<PeerId, PeerMetadata>,
    /// Send half for the command channel.
    ///
    /// This is kept so that a new [TransactionsHandle] can be created at any time.
    command_tx: mpsc::UnboundedSender<TransactionsCommand>,
    /// Incoming commands from [`TransactionsHandle`].
    ///
    /// This will only receive commands if a user manually sends a command to the manager through
    /// the [TransactionsHandle] to interact with this type directly.
    command_rx: UnboundedReceiverStream<TransactionsCommand>,
    /// A stream that yields new __pending__ transactions.
    ///
    /// A transaction is considered __pending__ if it is executable on the current state of the
    /// chain. In other words, this only yields transactions that satisfy all consensus
    /// requirements, these include:
    ///   - no nonce gaps
    ///   - all dynamic fee requirements are (currently) met
    ///   - account has enough balance to cover the transaction's gas
    pending_transactions: ReceiverStream<TxHash>,
    /// Incoming events from the [`NetworkManager`](crate::NetworkManager).
    transaction_events: UnboundedMeteredReceiver<NetworkTransactionEvent>,
    /// TransactionsManager metrics
    metrics: TransactionsManagerMetrics,
}

impl<Pool: TransactionPool> TransactionsManager<Pool> {
    /// Sets up a new instance.
    ///
    /// Note: This expects an existing [`NetworkManager`](crate::NetworkManager) instance.
    pub fn new(
        network: NetworkHandle,
        pool: Pool,
        from_network: mpsc::UnboundedReceiver<NetworkTransactionEvent>,
        transactions_manager_config: TransactionsManagerConfig,
    ) -> Self {
        let network_events = network.event_listener();

        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let transaction_fetcher = TransactionFetcher::default().with_transaction_fetcher_config(
            &transactions_manager_config.transaction_fetcher_config,
        );

        // install a listener for new __pending__ transactions that are allowed to be propagated
        // over the network
        let pending = pool.pending_transactions_listener();
        let pending_pool_imports_info = PendingPoolImportsInfo::default();

        let metrics = TransactionsManagerMetrics::default();
        metrics
            .capacity_inflight_requests
            .increment(transaction_fetcher.info.max_inflight_requests as u64);
        metrics
            .capacity_pending_pool_imports
            .increment(pending_pool_imports_info.max_pending_pool_imports as u64);

        Self {
            pool,
            network,
            network_events,
            transaction_fetcher,
            transactions_by_peers: Default::default(),
            pool_imports: Default::default(),
            pending_pool_imports_info: PendingPoolImportsInfo::new(
                DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS,
            ),
            bad_imports: LruCache::new(
                NonZeroUsize::new(DEFAULT_CAPACITY_CACHE_BAD_IMPORTS).unwrap(),
            ),
            peers: Default::default(),
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            pending_transactions: ReceiverStream::new(pending),
            transaction_events: UnboundedMeteredReceiver::new(
                from_network,
                NETWORK_POOL_TRANSACTIONS_SCOPE,
            ),
            metrics,
        }
    }
}

// === impl TransactionsManager ===

impl<Pool> TransactionsManager<Pool>
where
    Pool: TransactionPool,
{
    /// Returns a new handle that can send commands to this type.
    pub fn handle(&self) -> TransactionsHandle {
        TransactionsHandle { manager_tx: self.command_tx.clone() }
    }
}

impl<Pool> TransactionsManager<Pool>
where
    Pool: TransactionPool + 'static,
{
    #[inline]
    fn update_fetch_metrics(&self) {
        let tx_fetcher = &self.transaction_fetcher;

        self.metrics.inflight_transaction_requests.set(tx_fetcher.inflight_requests.len() as f64);

        let hashes_pending_fetch = tx_fetcher.hashes_pending_fetch.len() as f64;
        let total_hashes = tx_fetcher.hashes_fetch_inflight_and_pending_fetch.len() as f64;

        self.metrics.hashes_pending_fetch.set(hashes_pending_fetch);
        self.metrics.hashes_inflight_transaction_requests.set(total_hashes - hashes_pending_fetch);
    }

    #[inline]
    fn update_poll_metrics(&self, start: Instant, poll_durations: TxManagerPollDurations) {
        let metrics = &self.metrics;

        let TxManagerPollDurations {
            acc_network_events,
            acc_pending_imports,
            acc_tx_events,
            acc_imported_txns,
            acc_fetch_events,
            acc_pending_fetch,
            acc_cmds,
        } = poll_durations;

        // update metrics for whole poll function
        metrics.duration_poll_tx_manager.set(start.elapsed().as_secs_f64());
        // update poll metrics for nested streams
        metrics.acc_duration_poll_network_events.set(acc_network_events.as_secs_f64());
        metrics.acc_duration_poll_pending_pool_imports.set(acc_pending_imports.as_secs_f64());
        metrics.acc_duration_poll_transaction_events.set(acc_tx_events.as_secs_f64());
        metrics.acc_duration_poll_imported_transactions.set(acc_imported_txns.as_secs_f64());
        metrics.acc_duration_poll_fetch_events.set(acc_fetch_events.as_secs_f64());
        metrics.acc_duration_fetch_pending_hashes.set(acc_pending_fetch.as_secs_f64());
        metrics.acc_duration_poll_commands.set(acc_cmds.as_secs_f64());
    }

    /// Request handler for an incoming request for transactions
    fn on_get_pooled_transactions(
        &mut self,
        peer_id: PeerId,
        request: GetPooledTransactions,
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    ) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            if self.network.tx_gossip_disabled() {
                let _ = response.send(Ok(PooledTransactions::default()));
                return
            }
            let transactions = self.pool.get_pooled_transaction_elements(
                request.0,
                GetPooledTransactionLimit::ResponseSizeSoftLimit(
                    self.transaction_fetcher.info.soft_limit_byte_size_pooled_transactions_response,
                ),
            );

            // we sent a response at which point we assume that the peer is aware of the
            // transactions
            peer.seen_transactions.extend(transactions.iter().map(|tx| *tx.hash()));

            let resp = PooledTransactions(transactions);
            let _ = response.send(Ok(resp));
        }
    }

    /// Invoked when transactions in the local mempool are considered __pending__.
    ///
    /// When a transaction in the local mempool is moved to the pending pool, we propagate them to
    /// connected peers over network using the `Transactions` and `NewPooledTransactionHashes`
    /// messages. The Transactions message relays complete transaction objects and is typically
    /// sent to a small, random fraction of connected peers.
    ///
    /// All other peers receive a notification of the transaction hash and can request the
    /// complete transaction object if it is unknown to them. The dissemination of complete
    /// transactions to a fraction of peers usually ensures that all nodes receive the transaction
    /// and won't need to request it.
    fn on_new_pending_transactions(&mut self, hashes: Vec<TxHash>) {
        // Nothing to propagate while initially syncing
        if self.network.is_initially_syncing() {
            return
        }
        if self.network.tx_gossip_disabled() {
            return
        }

        trace!(target: "net::tx", num_hashes=?hashes.len(), "Start propagating transactions");

        // This fetches all transaction from the pool, including the 4844 blob transactions but
        // __without__ their sidecar, because 4844 transactions are only ever announced as hashes.
        let propagated = self.propagate_transactions(
            self.pool.get_all(hashes).into_iter().map(PropagateTransaction::new).collect(),
        );

        // notify pool so events get fired
        self.pool.on_propagated(propagated);
    }

    /// Propagate the transactions to all connected peers either as full objects or hashes.
    ///
    /// The message for new pooled hashes depends on the negotiated version of the stream.
    /// See [NewPooledTransactionHashes]
    ///
    /// Note: EIP-4844 are disallowed from being broadcast in full and are only ever sent as hashes, see also <https://eips.ethereum.org/EIPS/eip-4844#networking>.
    fn propagate_transactions(
        &mut self,
        to_propagate: Vec<PropagateTransaction>,
    ) -> PropagatedTransactions {
        let mut propagated = PropagatedTransactions::default();
        if self.network.tx_gossip_disabled() {
            return propagated
        }

        // send full transactions to a fraction of the connected peers (square root of the total
        // number of connected peers)
        let max_num_full = (self.peers.len() as f64).sqrt() as usize + 1;

        // Note: Assuming ~random~ order due to random state of the peers map hasher
        for (peer_idx, (peer_id, peer)) in self.peers.iter_mut().enumerate() {
            // filter all transactions unknown to the peer
            let mut hashes = PooledTransactionsHashesBuilder::new(peer.version);
            let mut full_transactions = FullTransactionsBuilder::default();

            // Iterate through the transactions to propagate and fill the hashes and full
            // transaction lists, before deciding whether or not to send full transactions to the
            // peer.
            for tx in to_propagate.iter() {
                if peer.seen_transactions.insert(tx.hash()) {
                    hashes.push(tx);

                    // Do not send full 4844 transaction hashes to peers.
                    //
                    //  Nodes MUST NOT automatically broadcast blob transactions to their peers.
                    //  Instead, those transactions are only announced using
                    //  `NewPooledTransactionHashes` messages, and can then be manually requested
                    //  via `GetPooledTransactions`.
                    //
                    // From: <https://eips.ethereum.org/EIPS/eip-4844#networking>
                    if !tx.transaction.is_eip4844() {
                        full_transactions.push(tx);
                    }
                }
            }
            let mut new_pooled_hashes = hashes.build();

            if !new_pooled_hashes.is_empty() {
                // determine whether to send full tx objects or hashes. If there are no full
                // transactions, try to send hashes.
                if peer_idx > max_num_full || full_transactions.is_empty() {
                    // enforce tx soft limit per message for the (unlikely) event the number of
                    // hashes exceeds it
                    new_pooled_hashes.truncate(
                        SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE,
                    );

                    for hash in new_pooled_hashes.iter_hashes().copied() {
                        propagated.0.entry(hash).or_default().push(PropagateKind::Hash(*peer_id));
                    }

                    trace!(target: "net::tx", ?peer_id, num_txs=?new_pooled_hashes.len(), "Propagating tx hashes to peer");

                    // send hashes of transactions
                    self.network.send_transactions_hashes(*peer_id, new_pooled_hashes);
                } else {
                    let new_full_transactions = full_transactions.build();

                    for tx in new_full_transactions.iter() {
                        propagated
                            .0
                            .entry(tx.hash())
                            .or_default()
                            .push(PropagateKind::Full(*peer_id));
                    }

                    trace!(target: "net::tx", ?peer_id, num_txs=?new_full_transactions.len(), "Propagating full transactions to peer");

                    // send full transactions
                    self.network.send_transactions(*peer_id, new_full_transactions);
                }
            }
        }

        // Update propagated transactions metrics
        self.metrics.propagated_transactions.increment(propagated.0.len() as u64);

        propagated
    }

    /// Propagate the full transactions to a specific peer
    ///
    /// Returns the propagated transactions
    fn propagate_full_transactions_to_peer(
        &mut self,
        txs: Vec<TxHash>,
        peer_id: PeerId,
    ) -> Option<PropagatedTransactions> {
        trace!(target: "net::tx", ?peer_id, "Propagating transactions to peer");

        let peer = self.peers.get_mut(&peer_id)?;
        let mut propagated = PropagatedTransactions::default();

        // filter all transactions unknown to the peer
        let mut full_transactions = FullTransactionsBuilder::default();

        let to_propagate = self
            .pool
            .get_all(txs)
            .into_iter()
            .filter(|tx| !tx.transaction.is_eip4844())
            .map(PropagateTransaction::new);

        // Iterate through the transactions to propagate and fill the hashes and full transaction
        for tx in to_propagate {
            if peer.seen_transactions.insert(tx.hash()) {
                full_transactions.push(&tx);
            }
        }

        if full_transactions.transactions.is_empty() {
            // nothing to propagate
            return None
        }

        let new_full_transactions = full_transactions.build();
        for tx in new_full_transactions.iter() {
            propagated.0.entry(tx.hash()).or_default().push(PropagateKind::Full(peer_id));
        }
        // send full transactions
        self.network.send_transactions(peer_id, new_full_transactions);

        // Update propagated transactions metrics
        self.metrics.propagated_transactions.increment(propagated.0.len() as u64);

        Some(propagated)
    }

    /// Propagate the transaction hashes to the given peer
    ///
    /// Note: This will only send the hashes for transactions that exist in the pool.
    fn propagate_hashes_to(&mut self, hashes: Vec<TxHash>, peer_id: PeerId) {
        trace!(target: "net::tx", "Start propagating transactions as hashes");

        // This fetches a transactions from the pool, including the blob transactions, which are
        // only ever sent as hashes.
        let propagated = {
            let Some(peer) = self.peers.get_mut(&peer_id) else {
                // no such peer
                return
            };

            let to_propagate: Vec<PropagateTransaction> =
                self.pool.get_all(hashes).into_iter().map(PropagateTransaction::new).collect();

            let mut propagated = PropagatedTransactions::default();

            // check if transaction is known to peer
            let mut hashes = PooledTransactionsHashesBuilder::new(peer.version);

            for tx in to_propagate {
                if !peer.seen_transactions.insert(tx.hash()) {
                    hashes.push(&tx);
                }
            }

            let new_pooled_hashes = hashes.build();

            if new_pooled_hashes.is_empty() {
                // nothing to propagate
                return
            }

            for hash in new_pooled_hashes.iter_hashes().copied() {
                propagated.0.entry(hash).or_default().push(PropagateKind::Hash(peer_id));
            }

            // send hashes of transactions
            self.network.send_transactions_hashes(peer_id, new_pooled_hashes);

            // Update propagated transactions metrics
            self.metrics.propagated_transactions.increment(propagated.0.len() as u64);

            propagated
        };

        // notify pool so events get fired
        self.pool.on_propagated(propagated);
    }

    /// Request handler for an incoming `NewPooledTransactionHashes`
    fn on_new_pooled_transaction_hashes(
        &mut self,
        peer_id: PeerId,
        msg: NewPooledTransactionHashes,
    ) {
        // If the node is initially syncing, ignore transactions
        if self.network.is_initially_syncing() {
            return
        }
        if self.network.tx_gossip_disabled() {
            return
        }

        // get handle to peer's session, if the session is still active
        let Some(peer) = self.peers.get_mut(&peer_id) else {
            trace!(
                peer_id=format!("{peer_id:#}"),
                msg=?msg,
                "discarding announcement from inactive peer"
            );

            return
        };
        let client = peer.client_version.clone();

        // keep track of the transactions the peer knows
        let mut count_txns_already_seen_by_peer = 0;
        for tx in msg.iter_hashes().copied() {
            if !peer.seen_transactions.insert(tx) {
                count_txns_already_seen_by_peer += 1;
            }
        }
        if count_txns_already_seen_by_peer > 0 {
            // this may occur if transactions are sent or announced to a peer, at the same time as
            // the peer sends/announces those hashes to us. this is because, marking
            // txns as seen by a peer is done optimistically upon sending them to the
            // peer.
            self.metrics.messages_with_hashes_already_seen_by_peer.increment(1);
            self.metrics
                .occurrences_hash_already_seen_by_peer
                .increment(count_txns_already_seen_by_peer);

            trace!(target: "net::tx",
                count_txns_already_seen_by_peer=%count_txns_already_seen_by_peer,
                peer_id=format!("{peer_id:#}"),
                client=?client,
                "Peer sent hashes that have already been marked as seen by peer"
            );

            self.report_already_seen(peer_id);
        }

        // 1. filter out spam
        let (validation_outcome, mut partially_valid_msg) =
            self.transaction_fetcher.filter_valid_message.partially_filter_valid_entries(msg);

        if let FilterOutcome::ReportPeer = validation_outcome {
            self.report_peer(peer_id, ReputationChangeKind::BadAnnouncement);
        }

        // 2. filter out known hashes
        //
        // known txns have already been successfully fetched or received over gossip.
        //
        // most hashes will be filtered out here since this the mempool protocol is a gossip
        // protocol, healthy peers will send many of the same hashes.
        //
        let hashes_count_pre_pool_filter = partially_valid_msg.len();
        self.pool.retain_unknown(&mut partially_valid_msg);
        if hashes_count_pre_pool_filter > partially_valid_msg.len() {
            let already_known_hashes_count =
                hashes_count_pre_pool_filter - partially_valid_msg.len();
            self.metrics
                .occurrences_hashes_already_in_pool
                .increment(already_known_hashes_count as u64);
        }

        if partially_valid_msg.is_empty() {
            // nothing to request
            return
        }

        // 3. filter out invalid entries (spam)
        //
        // validates messages with respect to the given network, e.g. allowed tx types
        //
        let (validation_outcome, mut valid_announcement_data) = if partially_valid_msg
            .msg_version()
            .expect("partially valid announcement should have version")
            .is_eth68()
        {
            // validate eth68 announcement data
            self.transaction_fetcher
                .filter_valid_message
                .filter_valid_entries_68(partially_valid_msg)
        } else {
            // validate eth66 announcement data
            self.transaction_fetcher
                .filter_valid_message
                .filter_valid_entries_66(partially_valid_msg)
        };

        if let FilterOutcome::ReportPeer = validation_outcome {
            self.report_peer(peer_id, ReputationChangeKind::BadAnnouncement);
        }

        if valid_announcement_data.is_empty() {
            // no valid announcement data
            return
        }

        // 4. filter out already seen unknown hashes
        //
        // seen hashes are already in the tx fetcher, pending fetch.
        //
        // for any seen hashes add the peer as fallback. unseen hashes are loaded into the tx
        // fetcher, hence they should be valid at this point.
        let bad_imports = &self.bad_imports;
        self.transaction_fetcher.filter_unseen_and_pending_hashes(
            &mut valid_announcement_data,
            |hash| bad_imports.contains(hash),
            &peer_id,
            |peer_id| self.peers.contains_key(&peer_id),
            &client,
        );

        if valid_announcement_data.is_empty() {
            // nothing to request
            return
        }

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            hashes_len=valid_announcement_data.iter().count(),
            hashes=?valid_announcement_data.keys().collect::<Vec<_>>(),
            msg_version=%valid_announcement_data.msg_version(),
            client_version=%client,
            "received previously unseen and pending hashes in announcement from peer"
        );

        // only send request for hashes to idle peer, otherwise buffer hashes storing peer as
        // fallback
        if !self.transaction_fetcher.is_idle(&peer_id) {
            // load message version before announcement data is destructed in packing
            let msg_version = valid_announcement_data.msg_version();
            let (hashes, _version) = valid_announcement_data.into_request_hashes();

            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                hashes=?*hashes,
                msg_version=%msg_version,
                client_version=%client,
                "buffering hashes announced by busy peer"
            );

            self.transaction_fetcher.buffer_hashes(hashes, Some(peer_id));

            return
        }

        // load message version before announcement data type is destructed in packing
        let msg_version = valid_announcement_data.msg_version();
        //
        // demand recommended soft limit on response, however the peer may enforce an arbitrary
        // limit on the response (2MB)
        //
        // request buffer is shrunk via call to pack request!
        let init_capacity_req =
            self.transaction_fetcher.approx_capacity_get_pooled_transactions_req(msg_version);
        let mut hashes_to_request = RequestTxHashes::with_capacity(init_capacity_req);
        let surplus_hashes =
            self.transaction_fetcher.pack_request(&mut hashes_to_request, valid_announcement_data);

        if !surplus_hashes.is_empty() {
            trace!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                surplus_hashes=?*surplus_hashes,
                msg_version=%msg_version,
                client_version=%client,
                "some hashes in announcement from peer didn't fit in `GetPooledTransactions` request, buffering surplus hashes"
            );

            self.transaction_fetcher.buffer_hashes(surplus_hashes, Some(peer_id));
        }

        trace!(target: "net::tx",
            peer_id=format!("{peer_id:#}"),
            hashes=?*hashes_to_request,
            msg_version=%msg_version,
            client_version=%client,
            "sending hashes in `GetPooledTransactions` request to peer's session"
        );

        // request the missing transactions
        //
        // get handle to peer's session again, at this point we know it exists
        let Some(peer) = self.peers.get_mut(&peer_id) else { return };
        let metrics = &self.metrics;
        if let Some(failed_to_request_hashes) =
            self.transaction_fetcher.request_transactions_from_peer(hashes_to_request, peer, || {
                metrics.egress_peer_channel_full.increment(1)
            })
        {
            let conn_eth_version = peer.version;

            debug!(target: "net::tx",
                peer_id=format!("{peer_id:#}"),
                failed_to_request_hashes=?*failed_to_request_hashes,
                conn_eth_version=%conn_eth_version,
                client_version=%client,
                "sending `GetPooledTransactions` request to peer's session failed, buffering hashes"
            );
            self.transaction_fetcher.buffer_hashes(failed_to_request_hashes, Some(peer_id));
        }
    }

    /// Handles dedicated transaction events related to the `eth` protocol.
    fn on_network_tx_event(&mut self, event: NetworkTransactionEvent) {
        match event {
            NetworkTransactionEvent::IncomingTransactions { peer_id, msg } => {
                // ensure we didn't receive any blob transactions as these are disallowed to be
                // broadcasted in full

                let has_blob_txs = msg.has_eip4844();

                let non_blob_txs = msg
                    .0
                    .into_iter()
                    .map(PooledTransactionsElement::try_from_broadcast)
                    .filter_map(Result::ok)
                    .collect::<PooledTransactions>();

                // mark the transactions as received
                self.transaction_fetcher.remove_hashes_from_transaction_fetcher(
                    non_blob_txs.iter().map(|tx| *tx.hash()),
                );

                self.import_transactions(peer_id, non_blob_txs, TransactionSource::Broadcast);

                if has_blob_txs {
                    debug!(target: "net::tx", ?peer_id, "received bad full blob transaction broadcast");
                    self.report_peer_bad_transactions(peer_id);
                }
            }
            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg } => {
                self.on_new_pooled_transaction_hashes(peer_id, msg)
            }
            NetworkTransactionEvent::GetPooledTransactions { peer_id, request, response } => {
                self.on_get_pooled_transactions(peer_id, request, response)
            }
            NetworkTransactionEvent::GetTransactionsHandle(response) => {
                let _ = response.send(Some(self.handle()));
            }
        }
    }

    /// Handles a command received from a detached [`TransactionsHandle`]
    fn on_command(&mut self, cmd: TransactionsCommand) {
        match cmd {
            TransactionsCommand::PropagateHash(hash) => {
                self.on_new_pending_transactions(vec![hash])
            }
            TransactionsCommand::PropagateHashesTo(hashes, peer) => {
                self.propagate_hashes_to(hashes, peer)
            }
            TransactionsCommand::GetActivePeers(tx) => {
                let peers = self.peers.keys().copied().collect::<HashSet<_>>();
                tx.send(peers).ok();
            }
            TransactionsCommand::PropagateTransactionsTo(_txs, _peer) => {
                if let Some(propagated) = self.propagate_full_transactions_to_peer(_txs, _peer) {
                    self.pool.on_propagated(propagated);
                }
            }
            TransactionsCommand::GetTransactionHashes { peers, tx } => {
                let mut res = HashMap::with_capacity(peers.len());
                for peer_id in peers {
                    let hashes = self
                        .peers
                        .get(&peer_id)
                        .map(|peer| peer.seen_transactions.iter().copied().collect::<HashSet<_>>())
                        .unwrap_or_default();
                    res.insert(peer_id, hashes);
                }
                tx.send(res).ok();
            }
            TransactionsCommand::GetPeerSender { peer_id, peer_request_sender } => {
                let sender = self.peers.get(&peer_id).map(|peer| peer.request_tx.clone());
                peer_request_sender.send(sender).ok();
            }
        }
    }

    /// Handles a received event related to common network events.
    fn on_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::SessionClosed { peer_id, .. } => {
                // remove the peer
                self.peers.remove(&peer_id);
            }
            NetworkEvent::SessionEstablished {
                peer_id, client_version, messages, version, ..
            } => {
                // Insert a new peer into the peerset.
                let peer = PeerMetadata::new(messages, version, client_version);
                let peer = match self.peers.entry(peer_id) {
                    Entry::Occupied(mut entry) => {
                        entry.insert(peer);
                        entry.into_mut()
                    }
                    Entry::Vacant(entry) => entry.insert(peer),
                };

                // Send a `NewPooledTransactionHashes` to the peer with up to
                // `SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE`
                // transactions in the pool.
                if self.network.is_initially_syncing() || self.network.tx_gossip_disabled() {
                    return
                }

                let pooled_txs = self.pool.pooled_transactions_max(
                    SOFT_LIMIT_COUNT_HASHES_IN_NEW_POOLED_TRANSACTIONS_BROADCAST_MESSAGE,
                );
                if pooled_txs.is_empty() {
                    // do not send a message if there are no transactions in the pool
                    return
                }

                let mut msg_builder = PooledTransactionsHashesBuilder::new(version);
                for pooled_tx in pooled_txs {
                    peer.seen_transactions.insert(*pooled_tx.hash());
                    msg_builder.push_pooled(pooled_tx);
                }

                let msg = msg_builder.build();
                self.network.send_transactions_hashes(peer_id, msg);
            }
            _ => {}
        }
    }

    /// Starts the import process for the given transactions.
    fn import_transactions(
        &mut self,
        peer_id: PeerId,
        transactions: PooledTransactions,
        source: TransactionSource,
    ) {
        // If the node is pipeline syncing, ignore transactions
        if self.network.is_initially_syncing() {
            return
        }
        if self.network.tx_gossip_disabled() {
            return
        }

        let mut transactions = transactions.0;

        let Some(peer) = self.peers.get_mut(&peer_id) else { return };

        // track that the peer knows these transaction, but only if this is a new broadcast.
        // If we received the transactions as the response to our `GetPooledTransactions``
        // requests (based on received `NewPooledTransactionHashes`) then we already
        // recorded the hashes as seen by this peer in `Self::on_new_pooled_transaction_hashes`.
        let mut num_already_seen_by_peer = 0;
        for tx in transactions.iter() {
            if source.is_broadcast() && !peer.seen_transactions.insert(*tx.hash()) {
                num_already_seen_by_peer += 1;
            }
        }

        // 1. filter out txns already inserted into pool
        let txns_count_pre_pool_filter = transactions.len();
        self.pool.retain_unknown(&mut transactions);
        if txns_count_pre_pool_filter > transactions.len() {
            let already_known_txns_count = txns_count_pre_pool_filter - transactions.len();
            self.metrics
                .occurrences_transactions_already_in_pool
                .increment(already_known_txns_count as u64);
        }

        // tracks the quality of the given transactions
        let mut has_bad_transactions = false;

        // 2. filter out transactions that are invalid or already pending import
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            // pre-size to avoid reallocations
            let mut new_txs = Vec::with_capacity(transactions.len());
            for tx in transactions {
                // recover transaction
                let tx = match tx.try_into_ecrecovered() {
                    Ok(tx) => tx,
                    Err(badtx) => {
                        trace!(target: "net::tx",
                            peer_id=format!("{peer_id:#}"),
                            hash=%badtx.hash(),
                            client_version=%peer.client_version,
                            "failed ecrecovery for transaction"
                        );
                        has_bad_transactions = true;
                        continue
                    }
                };

                match self.transactions_by_peers.entry(*tx.hash()) {
                    Entry::Occupied(mut entry) => {
                        // transaction was already inserted
                        entry.get_mut().push(peer_id);
                    }
                    Entry::Vacant(entry) => {
                        if !self.bad_imports.contains(tx.hash()) {
                            // this is a new transaction that should be imported into the pool
                            let pool_transaction = <Pool::Transaction as FromRecoveredPooledTransaction>::from_recovered_pooled_transaction(tx);
                            new_txs.push(pool_transaction);

                            entry.insert(vec![peer_id]);
                        } else {
                            trace!(target: "net::tx",
                                peer_id=format!("{peer_id:#}"),
                                hash=%tx.hash(),
                                client_version=%peer.client_version,
                                "received an invalid transaction from peer"
                            );
                            self.metrics.bad_imports.increment(1);
                        }
                    }
                }
            }
            new_txs.shrink_to_fit();

            // 3. import new transactions as a batch to minimize lock contention on the underlying
            // pool
            if !new_txs.is_empty() {
                let pool = self.pool.clone();
                // update metrics
                let metric_pending_pool_imports = self.metrics.pending_pool_imports.clone();
                metric_pending_pool_imports.increment(new_txs.len() as f64);

                // update self-monitoring info
                self.pending_pool_imports_info
                    .pending_pool_imports
                    .fetch_add(new_txs.len(), Ordering::Relaxed);
                let tx_manager_info_pending_pool_imports =
                    self.pending_pool_imports_info.pending_pool_imports.clone();

                let import = Box::pin(async move {
                    let added = new_txs.len();
                    let res = pool.add_external_transactions(new_txs).await;

                    // update metrics
                    metric_pending_pool_imports.decrement(added as f64);
                    // update self-monitoring info
                    tx_manager_info_pending_pool_imports.fetch_sub(added, Ordering::Relaxed);

                    res
                });

                self.pool_imports.push(import);
            }

            if num_already_seen_by_peer > 0 {
                self.metrics.messages_with_transactions_already_seen_by_peer.increment(1);
                self.metrics
                    .occurrences_of_transaction_already_seen_by_peer
                    .increment(num_already_seen_by_peer);
                trace!(target: "net::tx", num_txs=%num_already_seen_by_peer, ?peer_id, client=?peer.client_version, "Peer sent already seen transactions");
            }
        }

        if has_bad_transactions {
            // peer sent us invalid transactions
            self.report_peer_bad_transactions(peer_id)
        }

        if num_already_seen_by_peer > 0 {
            self.report_already_seen(peer_id);
        }
    }

    fn report_peer_bad_transactions(&self, peer_id: PeerId) {
        self.report_peer(peer_id, ReputationChangeKind::BadTransactions);
        self.metrics.reported_bad_transactions.increment(1);
    }

    fn report_peer(&self, peer_id: PeerId, kind: ReputationChangeKind) {
        trace!(target: "net::tx", ?peer_id, ?kind, "reporting reputation change");
        self.network.reputation_change(peer_id, kind);
    }

    fn on_request_error(&self, peer_id: PeerId, req_err: RequestError) {
        let kind = match req_err {
            RequestError::UnsupportedCapability => ReputationChangeKind::BadProtocol,
            RequestError::Timeout => ReputationChangeKind::Timeout,
            RequestError::ChannelClosed | RequestError::ConnectionDropped => {
                // peer is already disconnected
                return
            }
            RequestError::BadResponse => return self.report_peer_bad_transactions(peer_id),
        };
        self.report_peer(peer_id, kind);
    }

    fn report_already_seen(&self, peer_id: PeerId) {
        trace!(target: "net::tx", ?peer_id, "Penalizing peer for already seen transaction");
        self.network.reputation_change(peer_id, ReputationChangeKind::AlreadySeenTransaction);
    }

    /// Clear the transaction
    fn on_good_import(&mut self, hash: TxHash) {
        self.transactions_by_peers.remove(&hash);
    }

    /// Penalize the peers that sent the bad transaction and cache it to avoid fetching or
    /// importing it again.
    fn on_bad_import(&mut self, hash: TxHash) {
        if let Some(peers) = self.transactions_by_peers.remove(&hash) {
            for peer_id in peers {
                self.report_peer_bad_transactions(peer_id);
            }
        }
        self.transaction_fetcher.remove_hashes_from_transaction_fetcher([hash]);
        self.bad_imports.insert(hash);
    }

    /// Returns `true` if [`TransactionsManager`] has capacity to request pending hashes. Returns  
    /// `false` if [`TransactionsManager`] is operating close to full capacity.
    fn has_capacity_for_fetching_pending_hashes(&self) -> bool {
        self.pending_pool_imports_info
            .has_capacity(self.pending_pool_imports_info.max_pending_pool_imports) &&
            self.transaction_fetcher.has_capacity_for_fetching_pending_hashes()
    }
}

/// Measures the duration of executing the given code block. The duration is added to the given
/// accumulator value passed as a mutable reference.
macro_rules! duration_metered_exec {
    ($code:block, $acc:ident) => {
        let start = Instant::now();

        $code;

        *$acc += start.elapsed();
    };
}

#[derive(Debug, Default)]
struct TxManagerPollDurations {
    acc_network_events: Duration,
    acc_pending_imports: Duration,
    acc_tx_events: Duration,
    acc_imported_txns: Duration,
    acc_fetch_events: Duration,
    acc_pending_fetch: Duration,
    acc_cmds: Duration,
}

/// An endless future. Preemption ensure that future is non-blocking, nonetheless. See
/// [`crate::NetworkManager`] for more context on the design pattern.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<Pool> Future for TransactionsManager<Pool>
where
    Pool: TransactionPool + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let start = Instant::now();
        let mut poll_durations = TxManagerPollDurations::default();

        let this = self.get_mut();

        // If the budget is exhausted we manually yield back control to tokio. See
        // `NetworkManager` for more context on the design pattern.
        let mut budget = 1024;

        loop {
            let mut some_ready = false;

            let acc = &mut poll_durations.acc_network_events;
            duration_metered_exec!(
                {
                    // advance network/peer related events
                    if let Poll::Ready(Some(event)) = this.network_events.poll_next_unpin(cx) {
                        this.on_network_event(event);
                        some_ready = true;
                    }
                },
                acc
            );

            let acc = &mut poll_durations.acc_pending_fetch;
            duration_metered_exec!(
                {
                    if this.has_capacity_for_fetching_pending_hashes() {
                        // try drain transaction hashes pending fetch
                        let info = &this.pending_pool_imports_info;
                        let max_pending_pool_imports = info.max_pending_pool_imports;
                        let has_capacity_wrt_pending_pool_imports =
                            |divisor| info.has_capacity(max_pending_pool_imports / divisor);

                        let metrics = &this.metrics;
                        let metrics_increment_egress_peer_channel_full =
                            || metrics.egress_peer_channel_full.increment(1);

                        this.transaction_fetcher.on_fetch_pending_hashes(
                            &this.peers,
                            has_capacity_wrt_pending_pool_imports,
                            metrics_increment_egress_peer_channel_full,
                        );
                    }
                },
                acc
            );

            let acc = &mut poll_durations.acc_cmds;
            duration_metered_exec!(
                {
                    // advance commands
                    if let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                        this.on_command(cmd);
                        some_ready = true;
                    }
                },
                acc
            );

            let acc = &mut poll_durations.acc_tx_events;
            duration_metered_exec!(
                {
                    // advance incoming transaction events
                    if let Poll::Ready(Some(event)) = this.transaction_events.poll_next_unpin(cx) {
                        this.on_network_tx_event(event);
                        some_ready = true;
                    }
                },
                acc
            );

            let acc = &mut poll_durations.acc_fetch_events;
            duration_metered_exec!(
                {
                    this.update_fetch_metrics();

                    // advance fetching transaction events
                    if let Poll::Ready(Some(fetch_event)) =
                        this.transaction_fetcher.poll_next_unpin(cx)
                    {
                        match fetch_event {
                            FetchEvent::TransactionsFetched { peer_id, transactions } => {
                                this.import_transactions(
                                    peer_id,
                                    transactions,
                                    TransactionSource::Response,
                                );
                            }
                            FetchEvent::FetchError { peer_id, error } => {
                                trace!(target: "net::tx", ?peer_id, %error, "requesting transactions from peer failed");
                                this.on_request_error(peer_id, error);
                            }
                        }
                        some_ready = true;
                    }

                    this.update_fetch_metrics();
                },
                acc
            );

            let acc = &mut poll_durations.acc_pending_imports;
            duration_metered_exec!(
                {
                    // Advance all imports
                    if let Poll::Ready(Some(batch_import_res)) =
                        this.pool_imports.poll_next_unpin(cx)
                    {
                        for res in batch_import_res {
                            match res {
                                Ok(hash) => {
                                    this.on_good_import(hash);
                                }
                                Err(err) => {
                                    // if we're _currently_ syncing and the transaction is bad we
                                    // ignore it, otherwise we penalize the peer that sent the bad
                                    // transaction with the assumption that the peer should have
                                    // known that this transaction is bad. (e.g. consensus
                                    // rules)
                                    if err.is_bad_transaction() && !this.network.is_syncing() {
                                        debug!(target: "net::tx", %err, "bad pool transaction import");
                                        this.on_bad_import(err.hash);
                                        continue
                                    }
                                    this.on_good_import(err.hash);
                                }
                            }
                        }

                        some_ready = true;
                    }
                },
                acc
            );

            let acc = &mut poll_durations.acc_imported_txns;
            duration_metered_exec!(
                {
                    // drain new __pending__ transactions handle and propagate transactions.
                    // we drain this to batch the transactions in a single message.
                    // we don't expect this buffer to be large, since only pending transactions are
                    // emitted here.
                    let mut new_txs = Vec::new();
                    while let Poll::Ready(Some(hash)) =
                        this.pending_transactions.poll_next_unpin(cx)
                    {
                        new_txs.push(hash);
                    }
                    if !new_txs.is_empty() {
                        this.on_new_pending_transactions(new_txs);
                    }
                },
                acc
            );

            // all channels are fully drained and import futures pending
            if !some_ready {
                break
            }

            budget -= 1;
            if budget <= 0 {
                // Make sure we're woken up again
                cx.waker().wake_by_ref();
                break
            }
        }

        this.update_poll_metrics(start, poll_durations);

        Poll::Pending
    }
}

/// A transaction that's about to be propagated to multiple peers.
struct PropagateTransaction {
    size: usize,
    transaction: Arc<TransactionSigned>,
}

// === impl PropagateTransaction ===

impl PropagateTransaction {
    fn hash(&self) -> TxHash {
        self.transaction.hash()
    }

    /// Create a new instance from a pooled transaction
    fn new<T: PoolTransaction>(tx: Arc<ValidPoolTransaction<T>>) -> Self {
        let size = tx.encoded_length();
        let transaction = Arc::new(tx.transaction.to_recovered_transaction().into_signed());
        Self { size, transaction }
    }
}

/// Helper type for constructing the full transaction message that enforces the
/// [`DEFAULT_SOFT_LIMIT_BYTE_SIZE_TRANSACTIONS_BROADCAST_MESSAGE`].
#[derive(Default)]
struct FullTransactionsBuilder {
    total_size: usize,
    transactions: Vec<Arc<TransactionSigned>>,
}

// === impl FullTransactionsBuilder ===

impl FullTransactionsBuilder {
    /// Append a transaction to the list if the total message bytes size doesn't exceed the soft
    /// maximum target byte size. The limit is soft, meaning if one single transaction goes over
    /// the limit, it will be broadcasted in its own [`Transactions`] message. The same pattern is
    /// followed in filling a [`GetPooledTransactions`] request in
    /// [`TransactionFetcher::fill_request_from_hashes_pending_fetch`].
    fn push(&mut self, transaction: &PropagateTransaction) {
        let new_size = self.total_size + transaction.size;
        if new_size > DEFAULT_SOFT_LIMIT_BYTE_SIZE_TRANSACTIONS_BROADCAST_MESSAGE &&
            self.total_size > 0
        {
            return
        }

        self.total_size = new_size;
        self.transactions.push(Arc::clone(&transaction.transaction));
    }

    /// Returns whether or not any transactions are in the [FullTransactionsBuilder].
    fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// returns the list of transactions.
    fn build(self) -> Vec<Arc<TransactionSigned>> {
        self.transactions
    }
}

/// A helper type to create the pooled transactions message based on the negotiated version of the
/// session with the peer
enum PooledTransactionsHashesBuilder {
    Eth66(NewPooledTransactionHashes66),
    Eth68(NewPooledTransactionHashes68),
}

// === impl PooledTransactionsHashesBuilder ===

impl PooledTransactionsHashesBuilder {
    /// Push a transaction from the pool to the list.
    fn push_pooled<T: PoolTransaction>(&mut self, pooled_tx: Arc<ValidPoolTransaction<T>>) {
        match self {
            PooledTransactionsHashesBuilder::Eth66(msg) => msg.0.push(*pooled_tx.hash()),
            PooledTransactionsHashesBuilder::Eth68(msg) => {
                msg.hashes.push(*pooled_tx.hash());
                msg.sizes.push(pooled_tx.encoded_length());
                msg.types.push(pooled_tx.transaction.tx_type());
            }
        }
    }

    fn push(&mut self, tx: &PropagateTransaction) {
        match self {
            PooledTransactionsHashesBuilder::Eth66(msg) => msg.0.push(tx.hash()),
            PooledTransactionsHashesBuilder::Eth68(msg) => {
                msg.hashes.push(tx.hash());
                msg.sizes.push(tx.size);
                msg.types.push(tx.transaction.tx_type().into());
            }
        }
    }

    /// Create a builder for the negotiated version of the peer's session
    fn new(version: EthVersion) -> Self {
        match version {
            EthVersion::Eth66 | EthVersion::Eth67 => {
                PooledTransactionsHashesBuilder::Eth66(Default::default())
            }
            EthVersion::Eth68 => PooledTransactionsHashesBuilder::Eth68(Default::default()),
        }
    }

    fn build(self) -> NewPooledTransactionHashes {
        match self {
            PooledTransactionsHashesBuilder::Eth66(msg) => msg.into(),
            PooledTransactionsHashesBuilder::Eth68(msg) => msg.into(),
        }
    }
}

/// How we received the transactions.
enum TransactionSource {
    /// Transactions were broadcast to us via [`Transactions`] message.
    Broadcast,
    /// Transactions were sent as the response of [`fetcher::GetPooledTxRequest`] issued by us.
    Response,
}

// === impl TransactionSource ===

impl TransactionSource {
    /// Whether the transaction were sent as broadcast.
    fn is_broadcast(&self) -> bool {
        matches!(self, TransactionSource::Broadcast)
    }
}

/// Tracks a single peer in the context of [`TransactionsManager`].
#[derive(Debug)]
pub struct PeerMetadata {
    /// Optimistically keeps track of transactions that we know the peer has seen. Optimistic, in
    /// the sense that transactions are preemptively marked as seen by peer when they are sent to
    /// the peer.
    seen_transactions: LruCache<TxHash>,
    /// A communication channel directly to the peer's session task.
    request_tx: PeerRequestSender,
    /// negotiated version of the session.
    version: EthVersion,
    /// The peer's client version.
    client_version: Arc<str>,
}

impl PeerMetadata {
    /// Returns a new instance of [`PeerMetadata`].
    fn new(request_tx: PeerRequestSender, version: EthVersion, client_version: Arc<str>) -> Self {
        Self {
            seen_transactions: LruCache::new(
                NonZeroUsize::new(DEFAULT_CAPACITY_CACHE_SEEN_BY_PEER).expect("infallible"),
            ),
            request_tx,
            version,
            client_version,
        }
    }
}

/// Commands to send to the [`TransactionsManager`]
#[derive(Debug)]
enum TransactionsCommand {
    /// Propagate a transaction hash to the network.
    PropagateHash(B256),
    /// Propagate transaction hashes to a specific peer.
    PropagateHashesTo(Vec<B256>, PeerId),
    /// Request the list of active peer IDs from the [`TransactionsManager`].
    GetActivePeers(oneshot::Sender<HashSet<PeerId>>),
    /// Propagate a collection of full transactions to a specific peer.
    PropagateTransactionsTo(Vec<TxHash>, PeerId),
    /// Request transaction hashes known by specific peers from the [`TransactionsManager`].
    GetTransactionHashes {
        peers: Vec<PeerId>,
        tx: oneshot::Sender<HashMap<PeerId, HashSet<TxHash>>>,
    },
    /// Requests a clone of the sender sender channel to the peer.
    GetPeerSender {
        peer_id: PeerId,
        peer_request_sender: oneshot::Sender<Option<PeerRequestSender>>,
    },
}

/// All events related to transactions emitted by the network.
#[derive(Debug)]
pub enum NetworkTransactionEvent {
    /// Represents the event of receiving a list of transactions from a peer.
    ///
    /// This indicates transactions that were broadcasted to us from the peer.
    IncomingTransactions {
        /// The ID of the peer from which the transactions were received.
        peer_id: PeerId,
        /// The received transactions.
        msg: Transactions,
    },
    /// Represents the event of receiving a list of transaction hashes from a peer.
    IncomingPooledTransactionHashes {
        /// The ID of the peer from which the transaction hashes were received.
        peer_id: PeerId,
        /// The received new pooled transaction hashes.
        msg: NewPooledTransactionHashes,
    },
    /// Represents the event of receiving a `GetPooledTransactions` request from a peer.
    GetPooledTransactions {
        /// The ID of the peer from which the request was received.
        peer_id: PeerId,
        /// The received `GetPooledTransactions` request.
        request: GetPooledTransactions,
        /// The sender for responding to the request with a result of `PooledTransactions`.
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    },
    /// Represents the event of receiving a `GetTransactionsHandle` request.
    GetTransactionsHandle(oneshot::Sender<Option<TransactionsHandle>>),
}

/// Tracks stats about the [`TransactionsManager`].
#[derive(Debug)]
pub struct PendingPoolImportsInfo {
    /// Number of transactions about to be inserted into the pool.
    pending_pool_imports: Arc<AtomicUsize>,
    /// Max number of transactions allowed to be imported concurrently.
    max_pending_pool_imports: usize,
}

impl PendingPoolImportsInfo {
    /// Returns a new [`PendingPoolImportsInfo`].
    pub fn new(max_pending_pool_imports: usize) -> Self {
        Self { pending_pool_imports: Arc::new(AtomicUsize::default()), max_pending_pool_imports }
    }

    /// Returns `true` if the number of pool imports is under a given tolerated max.
    pub fn has_capacity(&self, max_pending_pool_imports: usize) -> bool {
        self.pending_pool_imports.load(Ordering::Relaxed) < max_pending_pool_imports
    }
}

impl Default for PendingPoolImportsInfo {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::Testnet, NetworkConfigBuilder, NetworkManager};
    use alloy_rlp::Decodable;
    use constants::tx_fetcher::DEFAULT_MAX_COUNT_FALLBACK_PEERS;
    use futures::FutureExt;
    use reth_interfaces::sync::{NetworkSyncUpdater, SyncState};
    use reth_network_api::NetworkInfo;
    use reth_primitives::hex;
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::test_utils::{testing_pool, MockTransaction};
    use secp256k1::SecretKey;
    use std::{future::poll_fn, hash};
    use tests::fetcher::TxFetchMetadata;

    async fn new_tx_manager() -> TransactionsManager<impl TransactionPool> {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let client = NoopProvider::default();

        let config = NetworkConfigBuilder::new(secret_key)
            // let OS choose port
            .listener_port(0)
            .disable_discovery()
            .build(client);

        let pool = testing_pool();

        let transactions_manager_config = config.transactions_manager_config.clone();
        let (_network_handle, _network, transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone(), transactions_manager_config)
            .split_with_handle();

        transactions
    }

    pub(super) fn default_cache<T: hash::Hash + Eq>() -> LruCache<T> {
        let limit = NonZeroUsize::new(DEFAULT_MAX_COUNT_FALLBACK_PEERS.into()).unwrap();
        LruCache::new(limit)
    }

    // Returns (peer, channel-to-send-get-pooled-tx-response-on).
    pub(super) fn new_mock_session(
        peer_id: PeerId,
        version: EthVersion,
    ) -> (PeerMetadata, mpsc::Receiver<PeerRequest>) {
        let (to_mock_session_tx, to_mock_session_rx) = mpsc::channel(1);

        (
            PeerMetadata::new(
                PeerRequestSender::new(peer_id, to_mock_session_tx),
                version,
                Arc::from(""),
            ),
            to_mock_session_rx,
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_ignored_tx_broadcasts_while_initially_syncing() {
        reth_tracing::init_test_tracing();
        let net = Testnet::create(3).await;

        let mut handles = net.handles();
        let handle0 = handles.next().unwrap();
        let handle1 = handles.next().unwrap();

        drop(handles);
        let handle = net.spawn();

        let listener0 = handle0.event_listener();
        handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let client = NoopProvider::default();
        let pool = testing_pool();
        let config = NetworkConfigBuilder::new(secret_key)
            .disable_discovery()
            .listener_port(0)
            .build(client);
        let transactions_manager_config = config.transactions_manager_config.clone();
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone(), transactions_manager_config)
            .split_with_handle();

        tokio::task::spawn(network);

        // go to syncing (pipeline sync)
        network_handle.update_sync_state(SyncState::Syncing);
        assert!(NetworkInfo::is_syncing(&network_handle));
        assert!(NetworkInfo::is_initially_syncing(&network_handle));

        // wait for all initiator connections
        let mut established = listener0.take(2);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    messages,
                    status,
                    version,
                } => {
                    // to insert a new peer in transactions peerset
                    transactions.on_network_event(NetworkEvent::SessionEstablished {
                        peer_id,
                        remote_addr,
                        client_version,
                        capabilities,
                        messages,
                        status,
                        version,
                    })
                }
                NetworkEvent::PeerAdded(_peer_id) => continue,
                ev => {
                    panic!("unexpected event {ev:?}")
                }
            }
        }
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let signed_tx = TransactionSigned::decode(&mut &input[..]).unwrap();
        transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
            peer_id: *handle1.peer_id(),
            msg: Transactions(vec![signed_tx.clone()]),
        });
        poll_fn(|cx| {
            let _ = transactions.poll_unpin(cx);
            Poll::Ready(())
        })
        .await;
        assert!(pool.is_empty());
        handle.terminate().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_tx_broadcasts_through_two_syncs() {
        reth_tracing::init_test_tracing();
        let net = Testnet::create(3).await;

        let mut handles = net.handles();
        let handle0 = handles.next().unwrap();
        let handle1 = handles.next().unwrap();

        drop(handles);
        let handle = net.spawn();

        let listener0 = handle0.event_listener();
        handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let client = NoopProvider::default();
        let pool = testing_pool();
        let config = NetworkConfigBuilder::new(secret_key)
            .disable_discovery()
            .listener_port(0)
            .build(client);
        let transactions_manager_config = config.transactions_manager_config.clone();
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone(), transactions_manager_config)
            .split_with_handle();

        tokio::task::spawn(network);

        // go to syncing (pipeline sync) to idle and then to syncing (live)
        network_handle.update_sync_state(SyncState::Syncing);
        assert!(NetworkInfo::is_syncing(&network_handle));
        network_handle.update_sync_state(SyncState::Idle);
        assert!(!NetworkInfo::is_syncing(&network_handle));
        network_handle.update_sync_state(SyncState::Syncing);
        assert!(NetworkInfo::is_syncing(&network_handle));

        // wait for all initiator connections
        let mut established = listener0.take(2);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    messages,
                    status,
                    version,
                } => {
                    // to insert a new peer in transactions peerset
                    transactions.on_network_event(NetworkEvent::SessionEstablished {
                        peer_id,
                        remote_addr,
                        client_version,
                        capabilities,
                        messages,
                        status,
                        version,
                    })
                }
                NetworkEvent::PeerAdded(_peer_id) => continue,
                ev => {
                    panic!("unexpected event {ev:?}")
                }
            }
        }
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let signed_tx = TransactionSigned::decode(&mut &input[..]).unwrap();
        transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
            peer_id: *handle1.peer_id(),
            msg: Transactions(vec![signed_tx.clone()]),
        });
        poll_fn(|cx| {
            let _ = transactions.poll_unpin(cx);
            Poll::Ready(())
        })
        .await;
        assert!(!NetworkInfo::is_initially_syncing(&network_handle));
        assert!(NetworkInfo::is_syncing(&network_handle));
        assert!(!pool.is_empty());
        handle.terminate().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_incoming_transactions() {
        reth_tracing::init_test_tracing();
        let net = Testnet::create(3).await;

        let mut handles = net.handles();
        let handle0 = handles.next().unwrap();
        let handle1 = handles.next().unwrap();

        drop(handles);
        let handle = net.spawn();

        let listener0 = handle0.event_listener();

        handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let client = NoopProvider::default();
        let pool = testing_pool();
        let config = NetworkConfigBuilder::new(secret_key)
            .disable_discovery()
            .listener_port(0)
            .build(client);
        let transactions_manager_config = config.transactions_manager_config.clone();
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone(), transactions_manager_config)
            .split_with_handle();
        tokio::task::spawn(network);

        network_handle.update_sync_state(SyncState::Idle);

        assert!(!NetworkInfo::is_syncing(&network_handle));

        // wait for all initiator connections
        let mut established = listener0.take(2);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    messages,
                    status,
                    version,
                } => {
                    // to insert a new peer in transactions peerset
                    transactions.on_network_event(NetworkEvent::SessionEstablished {
                        peer_id,
                        remote_addr,
                        client_version,
                        capabilities,
                        messages,
                        status,
                        version,
                    })
                }
                NetworkEvent::PeerAdded(_peer_id) => continue,
                ev => {
                    panic!("unexpected event {ev:?}")
                }
            }
        }
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let signed_tx = TransactionSigned::decode(&mut &input[..]).unwrap();
        transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
            peer_id: *handle1.peer_id(),
            msg: Transactions(vec![signed_tx.clone()]),
        });
        assert_eq!(
            *handle1.peer_id(),
            transactions.transactions_by_peers.get(&signed_tx.hash()).unwrap()[0]
        );

        // advance the transaction manager future
        poll_fn(|cx| {
            let _ = transactions.poll_unpin(cx);
            Poll::Ready(())
        })
        .await;

        assert!(!pool.is_empty());
        assert!(pool.get(&signed_tx.hash).is_some());
        handle.terminate().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_on_get_pooled_transactions_network() {
        reth_tracing::init_test_tracing();
        let net = Testnet::create(2).await;

        let mut handles = net.handles();
        let handle0 = handles.next().unwrap();
        let handle1 = handles.next().unwrap();

        drop(handles);
        let handle = net.spawn();

        let listener0 = handle0.event_listener();

        handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let client = NoopProvider::default();
        let pool = testing_pool();
        let config = NetworkConfigBuilder::new(secret_key)
            .disable_discovery()
            .listener_port(0)
            .build(client);
        let transactions_manager_config = config.transactions_manager_config.clone();
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone(), transactions_manager_config)
            .split_with_handle();
        tokio::task::spawn(network);

        network_handle.update_sync_state(SyncState::Idle);

        assert!(!NetworkInfo::is_syncing(&network_handle));

        // wait for all initiator connections
        let mut established = listener0.take(2);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    messages,
                    status,
                    version,
                } => transactions.on_network_event(NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    messages,
                    status,
                    version,
                }),
                NetworkEvent::PeerAdded(_peer_id) => continue,
                ev => {
                    panic!("unexpected event {ev:?}")
                }
            }
        }
        handle.terminate().await;

        let tx = MockTransaction::eip1559();
        let _ = transactions
            .pool
            .add_transaction(reth_transaction_pool::TransactionOrigin::External, tx.clone())
            .await;

        let request = GetPooledTransactions(vec![tx.get_hash()]);

        let (send, receive) = oneshot::channel::<RequestResult<PooledTransactions>>();

        transactions.on_network_tx_event(NetworkTransactionEvent::GetPooledTransactions {
            peer_id: *handle1.peer_id(),
            request,
            response: send,
        });

        match receive.await.unwrap() {
            Ok(PooledTransactions(transactions)) => {
                assert_eq!(transactions.len(), 1);
            }
            Err(e) => {
                panic!("error: {:?}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_max_retries_tx_request() {
        reth_tracing::init_test_tracing();

        let mut tx_manager = new_tx_manager().await;
        let tx_fetcher = &mut tx_manager.transaction_fetcher;

        let peer_id_1 = PeerId::new([1; 64]);
        let peer_id_2 = PeerId::new([2; 64]);
        let eth_version = EthVersion::Eth66;
        let seen_hashes = [B256::from_slice(&[1; 32]), B256::from_slice(&[2; 32])];

        let (mut peer_1, mut to_mock_session_rx) = new_mock_session(peer_id_1, eth_version);
        // mark hashes as seen by peer so it can fish them out from the cache for hashes pending
        // fetch
        peer_1.seen_transactions.insert(seen_hashes[0]);
        peer_1.seen_transactions.insert(seen_hashes[1]);
        tx_manager.peers.insert(peer_id_1, peer_1);

        // hashes are seen and currently not inflight, with one fallback peer, and are buffered
        // for first retry in reverse order to make index 0 lru
        let retries = 1;
        let mut backups = default_cache();
        backups.insert(peer_id_1);
        tx_fetcher
            .hashes_fetch_inflight_and_pending_fetch
            .insert(seen_hashes[1], TxFetchMetadata::new(retries, backups.clone(), None));
        tx_fetcher
            .hashes_fetch_inflight_and_pending_fetch
            .insert(seen_hashes[0], TxFetchMetadata::new(retries, backups, None));
        tx_fetcher.hashes_pending_fetch.insert(seen_hashes[1]);
        tx_fetcher.hashes_pending_fetch.insert(seen_hashes[0]);

        // peer_1 is idle
        assert!(tx_fetcher.is_idle(&peer_id_1));

        // sends request for buffered hashes to peer_1
        tx_fetcher.on_fetch_pending_hashes(&tx_manager.peers, |_| true, || ());

        let tx_fetcher = &mut tx_manager.transaction_fetcher;

        assert!(tx_fetcher.hashes_pending_fetch.is_empty());
        // as long as request is in inflight peer_1 is not idle
        assert!(!tx_fetcher.is_idle(&peer_id_1));

        // mock session of peer_1 receives request
        let req = to_mock_session_rx
            .recv()
            .await
            .expect("peer_1 session should receive request with buffered hashes");
        let PeerRequest::GetPooledTransactions { request, response } = req else { unreachable!() };
        let GetPooledTransactions(hashes) = request;

        assert_eq!(hashes, seen_hashes);

        // fail request to peer_1
        response
            .send(Err(RequestError::BadResponse))
            .expect("should send peer_1 response to tx manager");
        let Some(FetchEvent::FetchError { peer_id, .. }) = tx_fetcher.next().await else {
            unreachable!()
        };

        // request has resolved, peer_1 is idle again
        assert!(tx_fetcher.is_idle(&peer_id));
        // failing peer_1's request buffers requested hashes for retry
        assert_eq!(tx_fetcher.hashes_pending_fetch.len(), 2);

        let (peer_2, mut to_mock_session_rx) = new_mock_session(peer_id_2, eth_version);
        tx_manager.peers.insert(peer_id_2, peer_2);

        // peer_2 announces same hashes as peer_1
        let msg =
            NewPooledTransactionHashes::Eth66(NewPooledTransactionHashes66(seen_hashes.to_vec()));
        tx_manager.on_new_pooled_transaction_hashes(peer_id_2, msg);

        let tx_fetcher = &mut tx_manager.transaction_fetcher;

        // since hashes are already seen, no changes to length of unknown hashes
        assert_eq!(tx_fetcher.hashes_fetch_inflight_and_pending_fetch.len(), 2);
        // but hashes are taken out of buffer and packed into request to peer_2
        assert!(tx_fetcher.hashes_pending_fetch.is_empty());

        // mock session of peer_2 receives request
        let req = to_mock_session_rx
            .recv()
            .await
            .expect("peer_2 session should receive request with buffered hashes");
        let PeerRequest::GetPooledTransactions { response, .. } = req else { unreachable!() };

        // report failed request to tx manager
        response
            .send(Err(RequestError::BadResponse))
            .expect("should send peer_2 response to tx manager");
        let Some(FetchEvent::FetchError { .. }) = tx_fetcher.next().await else { unreachable!() };

        // `MAX_REQUEST_RETRIES_PER_TX_HASH`, 2, for hashes reached so this time won't be buffered
        // for retry
        assert!(tx_fetcher.hashes_pending_fetch.is_empty());
    }
}
