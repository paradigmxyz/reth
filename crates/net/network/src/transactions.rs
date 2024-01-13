//! Transactions management for the p2p network.

use crate::{
    cache::{LruCache, LruMap},
    manager::NetworkEvent,
    message::{PeerRequest, PeerRequestSender},
    metrics::{TransactionsManagerMetrics, NETWORK_POOL_TRANSACTIONS_SCOPE},
    NetworkEvents, NetworkHandle,
};
use futures::{stream::FuturesUnordered, Future, FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use reth_eth_wire::{
    EthVersion, GetPooledTransactions, NewPooledTransactionHashes, NewPooledTransactionHashes66,
    NewPooledTransactionHashes68, PooledTransactions, Transactions,
};
use reth_interfaces::{
    p2p::error::{RequestError, RequestResult},
    sync::SyncStateProvider,
};
use reth_metrics::common::mpsc::UnboundedMeteredReceiver;
use reth_network_api::{Peers, ReputationChangeKind};
use reth_primitives::{
    hex, FromRecoveredPooledTransaction, PeerId, PooledTransactionsElement, TransactionSigned,
    TxHash, B256,
};
use reth_transaction_pool::{
    error::PoolResult, GetPooledTransactionLimit, PoolTransaction, PropagateKind,
    PropagatedTransactions, TransactionPool, ValidPoolTransaction,
};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    fmt::Write,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, mpsc::error::TrySendError, oneshot, oneshot::error::RecvError};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};
use tracing::{debug, trace, warn};

/// Cache limit of transactions to keep track of for a single peer.
const PEER_TRANSACTION_CACHE_LIMIT: usize = 1024 * 10;

/// Cache limit of transactions waiting for idle peer to be fetched.
const MAX_CAPACITY_BUFFERED_HASHES: usize = 100 * GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES;

/// Soft limit for NewPooledTransactions
const NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT: usize = 4096;

/// The target size for the message of full transactions.
const MAX_FULL_TRANSACTIONS_PACKET_SIZE: usize = 100 * 1024;

/// Recommended soft limit for the number of hashes in a GetPooledTransactions message (8kb)
///
/// <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08>
const GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES: usize = 256;

/// Recommended limit for a response is 2MiB.
const RESPONSE_SIZE_LIMIT_BYTES: usize = 2 * 1048576;

/// Softlimit for the response size of a GetPooledTransactions message (2MB)
const GET_POOLED_TRANSACTION_SOFT_LIMIT_SIZE: GetPooledTransactionLimit =
    GetPooledTransactionLimit::SizeSoftLimit(2 * 1024 * 1024);

/// How many peers we keep track of for each missing transaction.
const MAX_ALTERNATIVE_PEERS_PER_TX: u8 =
    MAX_REQUEST_RETRIES_PER_TX_HASH + MARGINAL_FALLBACK_PEERS_PER_TX;

/// Marginal on fallback peers. If all fallback peers are idle, at most
/// [`MAX_REQUEST_RETRIES_PER_TX_HASH`] of them can ever be needed.
const MARGINAL_FALLBACK_PEERS_PER_TX: u8 = 1;

/// Maximum concurrent [`GetPooledTxRequest`]s to allow per peer.
const MAX_CONCURRENT_TX_REQUESTS_PER_PEER: u8 = 1;

/// Maximum concurrent [`GetPooledTxRequest`]s.
const MAX_CONCURRENT_TX_REQUESTS: u32 = 10000;

/// Maximum request retires per [`TxHash`]. Note, this is reset should the [`TxHash`] re-appear in
/// an announcement after it has been ejected from the hash buffer.
const MAX_REQUEST_RETRIES_PER_TX_HASH: u8 = 2;

/// The future for inserting a function into the pool
pub type PoolImportFuture = Pin<Box<dyn Future<Output = PoolResult<TxHash>> + Send + 'static>>;

/// Api to interact with [`TransactionsManager`] task.
#[derive(Debug, Clone)]
pub struct TransactionsHandle {
    /// Command channel to the [`TransactionsManager`]
    manager_tx: mpsc::UnboundedSender<TransactionsCommand>,
}

/// Implementation of the `TransactionsHandle` API.
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
/// This can be spawned to another task and is supposed to be run as background service while
/// [`TransactionsHandle`] is used as frontend to send commands to.
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
    /// Transactions that are currently imported into the `Pool`
    pool_imports: FuturesUnordered<PoolImportFuture>,
    /// All the connected peers.
    peers: HashMap<PeerId, Peer>,
    /// Send half for the command channel.
    command_tx: mpsc::UnboundedSender<TransactionsCommand>,
    /// Incoming commands from [`TransactionsHandle`].
    command_rx: UnboundedReceiverStream<TransactionsCommand>,
    /// Incoming commands from [`TransactionsHandle`].
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
    ) -> Self {
        let network_events = network.event_listener();
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        // install a listener for new pending transactions that are allowed to be propagated over
        // the network
        let pending = pool.pending_transactions_listener();

        Self {
            pool,
            network,
            network_events,
            transaction_fetcher: Default::default(),
            transactions_by_peers: Default::default(),
            pool_imports: Default::default(),
            peers: Default::default(),
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            pending_transactions: ReceiverStream::new(pending),
            transaction_events: UnboundedMeteredReceiver::new(
                from_network,
                NETWORK_POOL_TRANSACTIONS_SCOPE,
            ),
            metrics: Default::default(),
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
    fn update_import_metrics(&self) {
        self.metrics.pending_pool_imports.set(self.pool_imports.len() as f64);
    }

    #[inline]
    fn update_request_metrics(&self) {
        self.metrics
            .inflight_transaction_requests
            .set(self.transaction_fetcher.inflight_requests.len() as f64);
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
            let transactions = self
                .pool
                .get_pooled_transaction_elements(request.0, GET_POOLED_TRANSACTION_SOFT_LIMIT_SIZE);

            // we sent a response at which point we assume that the peer is aware of the
            // transactions
            peer.transactions.extend(transactions.iter().map(|tx| *tx.hash()));

            let resp = PooledTransactions(transactions);
            let _ = response.send(Ok(resp));
        }
    }

    /// Invoked when a new transaction is pending in the local pool.
    ///
    /// When new transactions appear in the pool, we propagate them to the network using the
    /// `Transactions` and `NewPooledTransactionHashes` messages. The Transactions message relays
    /// complete transaction objects and is typically sent to a small, random fraction of connected
    /// peers.
    ///
    /// All other peers receive a notification of the transaction hash and can request the
    /// complete transaction object if it is unknown to them. The dissemination of complete
    /// transactions to a fraction of peers usually ensures that all nodes receive the transaction
    /// and won't need to request it.
    fn on_new_transactions(&mut self, hashes: Vec<TxHash>) {
        // Nothing to propagate while initially syncing
        if self.network.is_initially_syncing() {
            return
        }
        if self.network.tx_gossip_disabled() {
            return
        }

        trace!(target: "net::tx", num_hashes=?hashes.len(), "Start propagating transactions");

        // This fetches all transaction from the pool, including the blob transactions, which are
        // only ever sent as hashes.
        let propagated = self.propagate_transactions(
            self.pool.get_all(hashes).into_iter().map(PropagateTransaction::new).collect(),
        );

        // notify pool so events get fired
        self.pool.on_propagated(propagated);
    }

    /// Propagate the transactions to all connected peers either as full objects or hashes
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

        // send full transactions to a fraction fo the connected peers (square root of the total
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
                if peer.transactions.insert(tx.hash()) {
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
                    new_pooled_hashes.truncate(NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT);

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
            if peer.transactions.insert(tx.hash()) {
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
                if peer.transactions.insert(tx.hash()) {
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
        mut msg: NewPooledTransactionHashes,
    ) {
        // If the node is initially syncing, ignore transactions
        if self.network.is_initially_syncing() {
            return
        }
        if self.network.tx_gossip_disabled() {
            return
        }

        let mut num_already_seen = 0;

        let Some(peer) = self.peers.get_mut(&peer_id) else { return };

        let hashes = msg.hashes_mut();
        // keep track of the transactions the peer knows
        for tx in hashes.iter().copied() {
            if !peer.transactions.insert(tx) {
                num_already_seen += 1;
            }
        }

        self.pool.retain_unknown(hashes);

        if hashes.is_empty() {
            // nothing to request
            return
        }

        let peer_id: PeerId = peer.request_tx.peer_id;

        // filter out hashes already in request, for those hashes add the peer as fallback
        self.transaction_fetcher.filter_unseen_hashes(hashes, peer_id);

        if hashes.is_empty() {
            // nothing to request
            return
        }

        debug!(target: "net::tx",
            peer_id=abbrev_hex(peer_id.as_ref()),
            hashes=abbrev_hex_hash_list(hashes),
            "received previously unseen hashes in announcement from peer"
        );

        // todo: choose idle status based on total inflight requests too?
        //let inflight_requests_count = *peer.inflight_requests_semaphore_rx.borrow();

        if !self.transaction_fetcher.is_idle(peer_id) {
            // since all hashes new at this point, no idle peer can exist that announced them
            trace!(target: "net::tx",
                peer_id=abbrev_hex(peer_id.as_ref()),
                hashes=abbrev_hex_hash_list(hashes),
                "buffering hashes announced by busy peer"
            );
            let hashes = msg.into_hashes();
            self.transaction_fetcher.buffer_hashes(hashes, Some(peer_id));
            return
        }

        // enforce recommended soft limit, however the peer may enforce an arbitrary limit on
        // the response (2MB)
        if let Some(left_over_hashes) = self.transaction_fetcher.pack_hashes(&mut msg, peer_id) {
            trace!(target: "net::tx",
                peer_id=abbrev_hex(peer_id.as_ref()),
                hashes=abbrev_hex_hash_list(&left_over_hashes),
                "some hashes in announcement from peer didn't fit in `GetPooledTransactions` request"
            );
            self.transaction_fetcher.buffer_hashes(left_over_hashes, Some(peer_id));
        }
        let hashes = msg.into_hashes();

        trace!(target: "net::tx",
            peer_id=abbrev_hex(peer_id.as_ref()),
            hashes=abbrev_hex_hash_list(&hashes),
            "sending hashes in `GetPooledTransactions` request to peer's session"
        );

        // request the missing transactions
        let metrics = &self.metrics;
        if let Some(failed_to_request_hashes) =
            self.transaction_fetcher.request_transactions_from_peer(hashes, peer, || {
                metrics.egress_peer_channel_full.increment(1)
            })
        {
            debug!(target: "net::tx",
                peer_id=abbrev_hex(peer_id.as_ref()),
                hashes=%abbrev_hex_hash_list(&failed_to_request_hashes),
                "sending `GetPooledTransactions` request to peer's session failed, buffering hashes"
            );
            self.transaction_fetcher.buffer_hashes(failed_to_request_hashes, Some(peer_id));
            return
        }

        if num_already_seen > 0 {
            self.metrics.messages_with_already_seen_hashes.increment(1);
            trace!(target: "net::tx", num_hashes=%num_already_seen, ?peer_id, client=?peer.client_version, "Peer sent already seen hashes");
            self.report_already_seen(peer_id);
        }
    }

    // Requests buffered hashes for which an idle peer exists.
    fn request_buffered_hashes(&mut self) {
        loop {
            let mut hashes = vec![];
            let Some(peer_id) = self.pop_any_idle_peer(&mut hashes) else {
                return;
            };

            debug_assert!(
                self.peers.contains_key(&peer_id),
                "broken invariant `peers` and `transaction-fetcher`"
            );

            // fill the request with other buffered hashes that have been announced by the peer
            if let Some(peer) = self.peers.get(&peer_id) {
                self.transaction_fetcher.fill_request_for_peer(&mut hashes, peer_id, peer.version);

                trace!(
                    target: "net::tx",
                    peer_id=abbrev_hex(peer_id.as_ref()),
                    hashes=abbrev_hex_hash_list(&hashes),
                    "requesting buffered hashes from idle peer"
                );

                // request the buffered missing transactions
                let metrics = &self.metrics;
                if let Some(failed_to_request_hashes) =
                    self.transaction_fetcher.request_transactions_from_peer(hashes, peer, || {
                        metrics.egress_peer_channel_full.increment(1)
                    })
                {
                    debug!(target: "net::tx",
                        peer_id=abbrev_hex(peer_id.as_ref()),
                        hashes=%abbrev_hex_hash_list(&failed_to_request_hashes),
                        "failed sending request to peer's session, buffering hashes"
                    );

                    self.transaction_fetcher.buffer_hashes(failed_to_request_hashes, Some(peer_id));
                    return
                }
            }
        }
    }

    /// Returns any idle peer for the given hash. Writes peer IDs of any ended sessions to buffer
    /// passed as parameter.
    fn get_idle_peer_for(
        &self,
        hash: TxHash,
        ended_sessions_buf: &mut Vec<PeerId>,
    ) -> Option<PeerId> {
        let (_, peers) = self.transaction_fetcher.unknown_hashes.get(&hash)?;

        for &peer_id in peers.iter() {
            // todo: check if session still active
            // todo: choose idle status based on total inflight requests too?
            // let inflight_requests_count = *peer.inflight_requests_semaphore_rx.borrow();
            if self.transaction_fetcher.is_idle(peer_id) {
                if self.peers.contains_key(&peer_id) {
                    return Some(peer_id)
                } else {
                    ended_sessions_buf.push(peer_id);
                }
            }
        }

        None
    }

    /// Returns any idle peer for any buffered unknown hash, and writes that hash to the request
    /// buffer passed as parameter.
    fn pop_any_idle_peer(&mut self, hashes: &mut Vec<TxHash>) -> Option<PeerId> {
        let mut ended_sessions = vec![];
        let mut buffered_hashes_iter = self.transaction_fetcher.buffered_hashes.iter();

        let idle_peer = loop {
            let Some(&hash) = buffered_hashes_iter.next() else { break None };

            let idle_peer = self.get_idle_peer_for(hash, &mut ended_sessions);
            for peer_id in ended_sessions.drain(..) {
                let (_, peers) = self.transaction_fetcher.unknown_hashes.get_mut(&hash)?;
                _ = peers.remove(&peer_id);
            }
            if idle_peer.is_some() {
                hashes.push(hash);
                break idle_peer
            }
        };

        let peer_id = &idle_peer?;

        let (_, peers) = self.transaction_fetcher.unknown_hashes.get_mut(&hashes[0])?;
        // pop peer from fallback peers
        _ = peers.remove(peer_id);
        // pop hash that is loaded in request buffer from buffered hashes
        drop(buffered_hashes_iter);
        self.transaction_fetcher.buffered_hashes.remove(&hashes[0]);

        idle_peer
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
                    .collect::<Vec<_>>();

                // mark the transactions as received
                self.transaction_fetcher.on_received_full_transactions_broadcast(
                    non_blob_txs.iter().map(|tx| *tx.hash()),
                );

                self.import_transactions(peer_id, non_blob_txs, TransactionSource::Broadcast);

                if has_blob_txs {
                    debug!(target: "net::tx", ?peer_id, "received bad full blob transaction broadcast");
                    self.report_peer(peer_id, ReputationChangeKind::BadTransactions);
                }
            }
            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg } => {
                self.on_new_pooled_transaction_hashes(peer_id, msg)
            }
            NetworkTransactionEvent::GetPooledTransactions { peer_id, request, response } => {
                self.on_get_pooled_transactions(peer_id, request, response)
            }
        }
    }

    /// Handles a command received from a detached [`TransactionsHandle`]
    fn on_command(&mut self, cmd: TransactionsCommand) {
        match cmd {
            TransactionsCommand::PropagateHash(hash) => self.on_new_transactions(vec![hash]),
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
                        .map(|peer| peer.transactions.iter().copied().collect::<HashSet<_>>())
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
                peer_id,
                client_version,
                messages,
                inflight_requests_semaphore_rx: _inflight_requests_semaphore_rx,
                version,
                ..
            } => {
                // insert a new peer into the peerset
                self.peers.insert(
                    peer_id,
                    Peer {
                        transactions: LruCache::new(
                            NonZeroUsize::new(PEER_TRANSACTION_CACHE_LIMIT).unwrap(),
                        ),
                        request_tx: messages,
                        //_inflight_requests_semaphore_rx,
                        version,
                        client_version,
                    },
                );

                // Send a `NewPooledTransactionHashes` to the peer with up to
                // `NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT` transactions in the
                // pool
                if !self.network.is_initially_syncing() {
                    if self.network.tx_gossip_disabled() {
                        return
                    }
                    let peer = self.peers.get_mut(&peer_id).expect("is present; qed");

                    let mut msg_builder = PooledTransactionsHashesBuilder::new(version);

                    let pooled_txs =
                        self.pool.pooled_transactions_max(NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT);
                    if pooled_txs.is_empty() {
                        // do not send a message if there are no transactions in the pool
                        return
                    }

                    for pooled_tx in pooled_txs.into_iter() {
                        peer.transactions.insert(*pooled_tx.hash());
                        msg_builder.push_pooled(pooled_tx);
                    }

                    let msg = msg_builder.build();
                    self.network.send_transactions_hashes(peer_id, msg);
                }
            }
            _ => {}
        }
    }

    /// Starts the import process for the given transactions.
    fn import_transactions(
        &mut self,
        peer_id: PeerId,
        transactions: Vec<PooledTransactionsElement>,
        source: TransactionSource,
    ) {
        // If the node is pipeline syncing, ignore transactions
        if self.network.is_initially_syncing() {
            return
        }
        if self.network.tx_gossip_disabled() {
            return
        }

        // tracks the quality of the given transactions
        let mut has_bad_transactions = false;
        let mut num_already_seen = 0;

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            for tx in transactions {
                // recover transaction
                let tx = if let Ok(tx) = tx.try_into_ecrecovered() {
                    tx
                } else {
                    has_bad_transactions = true;
                    continue
                };

                // track that the peer knows this transaction, but only if this is a new broadcast.
                // If we received the transactions as the response to our GetPooledTransactions
                // requests (based on received `NewPooledTransactionHashes`) then we already
                // recorded the hashes in [`Self::on_new_pooled_transaction_hashes`]
                if source.is_broadcast() && !peer.transactions.insert(*tx.hash()) {
                    num_already_seen += 1;
                }

                match self.transactions_by_peers.entry(*tx.hash()) {
                    Entry::Occupied(mut entry) => {
                        // transaction was already inserted
                        entry.get_mut().push(peer_id);
                    }
                    Entry::Vacant(entry) => {
                        // this is a new transaction that should be imported into the pool
                        let pool_transaction = <Pool::Transaction as FromRecoveredPooledTransaction>::from_recovered_pooled_transaction(tx);

                        let pool = self.pool.clone();

                        let import = Box::pin(async move {
                            pool.add_external_transaction(pool_transaction).await
                        });

                        self.pool_imports.push(import);
                        entry.insert(vec![peer_id]);
                    }
                }
            }

            if num_already_seen > 0 {
                self.metrics.messages_with_already_seen_transactions.increment(1);
                trace!(target: "net::tx", num_txs=%num_already_seen, ?peer_id, client=?peer.client_version, "Peer sent already seen transactions");
            }
        }

        if has_bad_transactions || num_already_seen > 0 {
            self.report_already_seen(peer_id);
        }
    }

    fn report_peer(&self, peer_id: PeerId, kind: ReputationChangeKind) {
        trace!(target: "net::tx", ?peer_id, ?kind, "reporting reputation change");
        self.network.reputation_change(peer_id, kind);
        self.metrics.reported_bad_transactions.increment(1);
    }

    fn on_request_error(&self, peer_id: PeerId, req_err: RequestError) {
        let kind = match req_err {
            RequestError::UnsupportedCapability => ReputationChangeKind::BadProtocol,
            RequestError::Timeout => ReputationChangeKind::Timeout,
            RequestError::ChannelClosed | RequestError::ConnectionDropped => {
                // peer is already disconnected
                return
            }
            RequestError::BadResponse => ReputationChangeKind::BadTransactions,
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

    /// Penalize the peers that sent the bad transaction
    fn on_bad_import(&mut self, hash: TxHash) {
        if let Some(peers) = self.transactions_by_peers.remove(&hash) {
            for peer_id in peers {
                self.report_peer(peer_id, ReputationChangeKind::BadTransactions);
            }
        }
    }
}

/// An endless future.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<Pool> Future for TransactionsManager<Pool>
where
    Pool: TransactionPool + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // drain network/peer related events
        while let Poll::Ready(Some(event)) = this.network_events.poll_next_unpin(cx) {
            this.on_network_event(event);
        }

        // drain commands
        while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
            this.on_command(cmd);
        }

        // drain incoming transaction events
        while let Poll::Ready(Some(event)) = this.transaction_events.poll_next_unpin(cx) {
            this.on_network_tx_event(event);
        }

        this.update_request_metrics();

        // drain fetching transaction events
        while let Poll::Ready(Some(fetch_event)) = this.transaction_fetcher.poll_next_unpin(cx) {
            match fetch_event {
                FetchEvent::TransactionsFetched { peer_id, transactions } => {
                    this.import_transactions(peer_id, transactions, TransactionSource::Response);
                }
                FetchEvent::FetchError { peer_id, error } => {
                    trace!(target: "net::tx", ?peer_id, ?error, "requesting transactions from peer failed");
                    this.on_request_error(peer_id, error);
                }
            }
        }

        // try drain buffered transactions
        this.request_buffered_hashes();

        this.update_request_metrics();
        this.update_import_metrics();

        // Advance all imports
        while let Poll::Ready(Some(import_res)) = this.pool_imports.poll_next_unpin(cx) {
            match import_res {
                Ok(hash) => {
                    this.on_good_import(hash);
                }
                Err(err) => {
                    // if we're _currently_ syncing and the transaction is bad we ignore it,
                    // otherwise we penalize the peer that sent the bad
                    // transaction with the assumption that the peer should have
                    // known that this transaction is bad. (e.g. consensus
                    // rules)
                    if err.is_bad_transaction() && !this.network.is_syncing() {
                        debug!(target: "net::tx", ?err, "bad pool transaction import");
                        this.on_bad_import(err.hash);
                        continue
                    }
                    this.on_good_import(err.hash);
                }
            }
        }

        this.update_import_metrics();

        // handle and propagate new transactions
        let mut new_txs = Vec::new();
        while let Poll::Ready(Some(hash)) = this.pending_transactions.poll_next_unpin(cx) {
            new_txs.push(hash);
        }
        if !new_txs.is_empty() {
            this.on_new_transactions(new_txs);
        }

        // all channels are fully drained and import futures pending

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
/// `MAX_FULL_TRANSACTIONS_PACKET_SIZE`
#[derive(Default)]
struct FullTransactionsBuilder {
    total_size: usize,
    transactions: Vec<Arc<TransactionSigned>>,
}

// === impl FullTransactionsBuilder ===

impl FullTransactionsBuilder {
    /// Append a transaction to the list if it doesn't exceed the maximum size.
    fn push(&mut self, transaction: &PropagateTransaction) {
        let new_size = self.total_size + transaction.size;
        if new_size > MAX_FULL_TRANSACTIONS_PACKET_SIZE {
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
    /// Transactions were sent as the response of [`GetPooledTxRequest`] issued by us.
    Response,
}

// === impl TransactionSource ===

impl TransactionSource {
    /// Whether the transaction were sent as broadcast.
    fn is_broadcast(&self) -> bool {
        matches!(self, TransactionSource::Broadcast)
    }
}

/// An inflight request for `PooledTransactions` from a peer
struct GetPooledTxRequest {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes
    requested_hashes: Vec<TxHash>,
    response: oneshot::Receiver<RequestResult<PooledTransactions>>,
}

struct GetPooledTxResponse {
    peer_id: PeerId,
    /// Transaction hashes that were requested, for cleanup purposes
    requested_hashes: Vec<TxHash>,
    result: Result<RequestResult<PooledTransactions>, RecvError>,
}

#[must_use = "futures do nothing unless polled"]
#[pin_project::pin_project]
struct GetPooledTxRequestFut {
    #[pin]
    inner: Option<GetPooledTxRequest>,
}

impl GetPooledTxRequestFut {
    #[inline]
    fn new(
        peer_id: PeerId,
        requested_hashes: Vec<TxHash>,
        response: oneshot::Receiver<RequestResult<PooledTransactions>>,
    ) -> Self {
        Self { inner: Some(GetPooledTxRequest { peer_id, requested_hashes, response }) }
    }
}

impl Future for GetPooledTxRequestFut {
    type Output = GetPooledTxResponse;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut req = self.as_mut().project().inner.take().expect("polled after completion");
        match req.response.poll_unpin(cx) {
            Poll::Ready(result) => Poll::Ready(GetPooledTxResponse {
                peer_id: req.peer_id,
                requested_hashes: req.requested_hashes,
                result,
            }),
            Poll::Pending => {
                self.project().inner.set(Some(req));
                Poll::Pending
            }
        }
    }
}

/// Tracks a single peer
#[derive(Debug)]
struct Peer {
    /// Keeps track of transactions that we know the peer has seen.
    transactions: LruCache<B256>,
    /// A communication channel directly to the peer's session task.
    request_tx: PeerRequestSender,
    /// Counting semaphore for inflight requests.
    //_inflight_requests_semaphore_rx: watch::Receiver<usize>,
    /// negotiated version of the session.
    version: EthVersion,
    /// The peer's client version.
    client_version: Arc<str>,
}

/// The type responsible for fetching missing transactions from peers.
///
/// This will keep track of unique transaction hashes that are currently being fetched and submits
/// new requests on announced hashes.
#[derive(Debug)]
#[pin_project]
struct TransactionFetcher {
    /// All peers to which a request for pooled transactions is currently active. Maps 1-1 to
    /// `inflight_requests`.
    active_peers: LruMap<PeerId, u8>,
    /// All currently active requests for pooled transactions.
    #[pin]
    inflight_requests: FuturesUnordered<GetPooledTxRequestFut>,
    /// Hashes that are awaiting fetch from an idle peer.
    buffered_hashes: LruCache<TxHash>,
    /// Tracks all hashes that are currently being fetched or are buffered, mapping them to
    /// request retries and last recently seen fallback peers (max one request try for any peer).
    unknown_hashes: HashMap<TxHash, (u8, LruCache<PeerId>)>,
}

// === impl TransactionFetcher ===

impl TransactionFetcher {
    /// Removes the specified hashes from inflight tracking.
    #[inline]
    fn remove_from_unknown_hashes<I>(&mut self, hashes: I)
    where
        I: IntoIterator<Item = TxHash>,
    {
        for hash in hashes {
            self.unknown_hashes.remove(&hash);
        }
    }

    /// Updates peer's activity status upon a resolved [`GetPooledTxRequest`].
    fn update_peer_activity(&mut self, resp: &GetPooledTxResponse) {
        let GetPooledTxResponse { peer_id, .. } = resp;

        debug_assert!(
            self.active_peers.get(peer_id).is_some(),
            "broken invariant `active_peers` and `inflight_requests`"
        );

        let mut remove = || -> bool {
            if let Some(inflight_count) = self.active_peers.get(peer_id) {
                if *inflight_count <= 1 {
                    return true
                }
                *inflight_count -= 1;
            }
            false
        };
        if remove() {
            self.active_peers.remove(peer_id);
        }
    }

    /// Returns `true` if peer is idle.
    fn is_idle(&self, peer_id: PeerId) -> bool {
        let Some(inflight_count) = self.active_peers.peek(&peer_id) else { return true };
        if *inflight_count < MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
            return true
        }
        false
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit. Returns left over hashes.
    fn pack_hashes(
        &mut self,
        msg: &mut NewPooledTransactionHashes,
        peer_id: PeerId,
    ) -> Option<Vec<TxHash>> {
        match msg {
            NewPooledTransactionHashes::Eth66(NewPooledTransactionHashes66(hashes)) => {
                self.pack_hashes_eth66(hashes, peer_id).map(|left_over| left_over.collect())
            }
            NewPooledTransactionHashes::Eth68(NewPooledTransactionHashes68 {
                sizes,
                hashes,
                ..
            }) => self.pack_hashes_eth68(hashes, sizes, peer_id),
        }
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit as defined by protocol version 66.
    /// Returns left over hashes.
    fn pack_hashes_eth66<'a>(
        &mut self,
        hashes: &'a mut Vec<TxHash>,
        peer_id: PeerId,
    ) -> Option<impl Iterator<Item = TxHash> + 'a> {
        if hashes.len() < GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES {
            self.fill_request_for_peer(hashes, peer_id, EthVersion::Eth66);
            return None
        }
        Some(hashes.drain(GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES..))
    }

    /// Packages hashes for [`GetPooledTxRequest`] up to limit as defined by protocol version 68.
    /// Returns left over hashes.
    fn pack_hashes_eth68(
        &mut self,
        hashes: &mut Vec<TxHash>,
        sizes: &[usize],
        peer_id: PeerId,
    ) -> Option<Vec<TxHash>> {
        let mut acc_size = 0;
        let first_left_over_index = sizes.iter().enumerate().find(|(_, size)| {
            if acc_size < RESPONSE_SIZE_LIMIT_BYTES {
                acc_size += **size;
                false
            } else {
                true
            }
        });

        let mut include_indices = vec![];

        let Some((i, _)) = first_left_over_index else {
            // all hashes included in request
            if acc_size < RESPONSE_SIZE_LIMIT_BYTES {
                self.fill_request_for_peer(hashes, peer_id, EthVersion::Eth68);
            }
            return None;
        };

        // some space will be left in response
        if acc_size < RESPONSE_SIZE_LIMIT_BYTES {
            for (j, size) in sizes[i..].iter().enumerate() {
                if *size <= RESPONSE_SIZE_LIMIT_BYTES - acc_size {
                    // tx will fit into response, include hash in request
                    include_indices.push(j);
                    acc_size += *size;
                }
            }
        }

        let mut left_over_hashes = hashes.drain(..i).collect::<Vec<_>>();
        let len_hashes = hashes.len();

        // re-insert hashes to include
        for (j, h) in include_indices.into_iter().enumerate() {
            // elements in left over hashes budge on every iteration due to remove
            let n = h + 1;
            let hash = left_over_hashes.remove(j - len_hashes - n);
            hashes.push(hash);
        }

        Some(left_over_hashes)
    }

    fn buffer_hashes_for_retry(&mut self, hashes: impl IntoIterator<Item = TxHash>) {
        self.buffer_hashes(hashes, None)
    }

    /// Buffers hashes. Only peers that haven't yet tried to request the hashes should be passed
    /// as `fallback_peer` parameter.
    fn buffer_hashes(
        &mut self,
        hashes: impl IntoIterator<Item = TxHash>,
        fallback_peer: Option<PeerId>,
    ) {
        let mut max_retried_hashes = vec![];

        for hash in hashes {
            // todo: enforce by adding new type UnknownTxHash
            debug_assert!(
                self.unknown_hashes.contains_key(&hash),
                "only hashes that are confirmed as unknown should be buffered"
            );

            let Some((retries, peers)) = self.unknown_hashes.get_mut(&hash) else { return };

            if let Some(peer_id) = fallback_peer {
                // peer has not yet requested hash
                peers.insert(peer_id);
            } else {
                // peer in caller's context has requested hash and is hence not eligible as
                // fallback peer.
                if *retries >= MAX_REQUEST_RETRIES_PER_TX_HASH {
                    warn!(target: "net::tx",
                        hash=abbrev_hex(hash.as_ref()),
                        retries=retries,
                        "retry limit for `GetPooledTransactions` requests reached for hash, dropping hash"
                    );
                    max_retried_hashes.push(hash);
                    continue;
                }
                *retries += 1;
            }
            if let (_, Some(evicted_hash)) =
                self.buffered_hashes.insert_with_eviction_feedback(hash)
            {
                _ = self.unknown_hashes.remove(&evicted_hash);
            }
        }

        self.remove_from_unknown_hashes(max_retried_hashes);
    }

    /// Removes the provided transaction hashes from the inflight requests set.
    ///
    /// This is called when we receive full transactions that are currently scheduled for fetching.
    #[inline]
    fn on_received_full_transactions_broadcast(
        &mut self,
        hashes: impl IntoIterator<Item = TxHash>,
    ) {
        self.remove_from_unknown_hashes(hashes)
    }

    fn filter_unseen_hashes(&mut self, announced_hashes: &mut Vec<TxHash>, peer_id: PeerId) {
        // 1. filter out inflight hashes, and register the peer as fallback for all inflight hashes
        announced_hashes.retain(|&hash| {
            match self.unknown_hashes.entry(hash) {
                Entry::Vacant(entry) => {
                    trace!(
                        target: "net::tx",
                        peer_id=abbrev_hex(peer_id.as_ref()),
                        hashes=abbrev_hex(hash.as_ref()),
                        "new hash seen in announcement by peer"
                    );
                    // todo: allow `MAX_ALTERNATIVE_PEERS_PER_TX` to be zero by Option<LruCache>
                    // for backups
                    if let Some(limit) = NonZeroUsize::new(MAX_ALTERNATIVE_PEERS_PER_TX.into()) {
                        entry.insert((0, LruCache::new(limit)));
                    }
                    true
                }
                Entry::Occupied(mut entry) => {
                    let (_, backups) = entry.get_mut();
                    // hash has been seen but is not inflight
                    if self.buffered_hashes.remove(&hash) {
                        return true
                    }
                    // hash has been seen and is in flight. store peer as fallback peer.
                    // todo: check if session is still active
                    backups.insert(peer_id);
                    false
                }
            }
        })
    }

    /// Requests the missing transactions from the announced hashes of the peer. Returns the
    /// requested hashes if concurrency limit is reached or if the request was fails to send over
    /// the channel to the peer's session task.
    ///
    /// This filters all announced hashes that are already in flight, and requests the missing,
    /// while marking the given peer as an alternative peer for the hashes that are already in
    /// flight.
    fn request_transactions_from_peer(
        &mut self,
        new_announced_hashes: Vec<TxHash>,
        peer: &Peer,
        metrics_increment_egress_peer_channel_full: impl FnOnce(),
    ) -> Option<Vec<TxHash>> {
        let peer_id: PeerId = peer.request_tx.peer_id;

        if self.active_peers.len() as u32 >= MAX_CONCURRENT_TX_REQUESTS {
            debug!(target: "net::tx",
                peer_id=abbrev_hex(peer_id.as_ref()),
                hashes=abbrev_hex_hash_list(&new_announced_hashes),
                limit=MAX_CONCURRENT_TX_REQUESTS,
                "limit for concurrent `GetPooledTransactions` requests reached, dropping request for hashes to peer"
            );
            return Some(new_announced_hashes)
        }

        let Some(inflight_count) = self.active_peers.get_or_insert(peer_id, || 0) else {
            warn!(target: "net::tx",
                peer_id=abbrev_hex(peer_id.as_ref()),
                hashes=abbrev_hex_hash_list(&new_announced_hashes),
                "failed to cache active peer in schnellru::LruMap, dropping request to peer"
            );
            return Some(new_announced_hashes)
        };

        if *inflight_count >= MAX_CONCURRENT_TX_REQUESTS_PER_PEER {
            debug!(target: "net::tx",
                peer_id=abbrev_hex(peer_id.as_ref()),
                hashes=abbrev_hex_hash_list(&new_announced_hashes),
                limit=MAX_CONCURRENT_TX_REQUESTS_PER_PEER,
                "limit for concurrent `GetPooledTransactions` requests per peer reached"
            );
            return Some(new_announced_hashes)
        }

        *inflight_count += 1;

        let (response, rx) = oneshot::channel();
        let req: PeerRequest = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(new_announced_hashes.clone()),
            response,
        };

        // try to send the request to the peer
        if let Err(err) = peer.request_tx.try_send(req) {
            // peer channel is full
            match err {
                TrySendError::Full(req) | TrySendError::Closed(req) => {
                    // need to do some cleanup so
                    let req = req.into_get_pooled_transactions().expect("is get pooled tx");

                    // we know that the peer is the only entry in the map, so we can remove all
                    self.remove_from_unknown_hashes(req.0);
                }
            }
            metrics_increment_egress_peer_channel_full();
            return Some(new_announced_hashes)
        } else {
            // remove requested hashes from buffered hashes
            debug_assert!(
                || -> bool {
                    for hash in &new_announced_hashes {
                        if self.buffered_hashes.contains(hash) {
                            return false
                        }
                    }
                    true
                }(),
                "broken invariant `buffered_hashes` and `unknown_hashes`"
            );

            // stores a new request future for the request
            self.inflight_requests.push(GetPooledTxRequestFut::new(
                peer_id,
                new_announced_hashes,
                rx,
            ))
        }

        None
    }

    /// Fills free space in request with hashes from buffer.
    fn fill_request_for_peer(
        &mut self,
        hashes: &mut Vec<TxHash>,
        peer_id: PeerId,
        version: EthVersion,
    ) {
        for hash in self.buffered_hashes.iter() {
            match version {
                EthVersion::Eth66 | EthVersion::Eth67 | EthVersion::Eth68 => {
                    if hashes.len() >= GET_POOLED_TRANSACTION_SOFT_LIMIT_NUM_HASHES {
                        return
                    }
                } /* EthVersion::Eth68 => return, todo: store eth68 TxHash with size metadata to
                   * pack request */
            }

            debug_assert!(!hashes.is_empty(), "expected request buffer to have at least one hash");

            if *hash == hashes[0] {
                continue;
            }
            if let Some((_, peers)) = self.unknown_hashes.get_mut(hash) {
                if peers.remove(&peer_id) {
                    hashes.push(*hash)
                }
            }
        }

        for hash in hashes {
            self.buffered_hashes.remove(hash);
        }
    }
}

impl Stream for TransactionFetcher {
    type Item = FetchEvent;

    /// Advances all inflight requests and returns the next event.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut().project();
        let res = this.inflight_requests.poll_next_unpin(cx);

        if let Poll::Ready(Some(response)) = res {
            // update peer activity, requests for buffered hashes can only be made to idle
            // fallback peers
            self.update_peer_activity(&response);

            let GetPooledTxResponse { peer_id, mut requested_hashes, result } = response;

            return match result {
                Ok(Ok(transactions)) => {
                    // clear received hashes
                    requested_hashes.retain(|requested_hash| {
                        if transactions.hashes().any(|hash| hash == requested_hash) {
                            // hash is now known, stop tracking
                            self.unknown_hashes.remove(requested_hash);
                            return false
                        }
                        true
                    });
                    // buffer left over hashes
                    self.buffer_hashes_for_retry(requested_hashes);

                    Poll::Ready(Some(FetchEvent::TransactionsFetched {
                        peer_id,
                        transactions: transactions.0,
                    }))
                }
                Ok(Err(req_err)) => {
                    self.buffer_hashes_for_retry(requested_hashes);
                    Poll::Ready(Some(FetchEvent::FetchError { peer_id, error: req_err }))
                }
                Err(_) => {
                    self.buffer_hashes_for_retry(requested_hashes);
                    // request channel closed/dropped
                    Poll::Ready(Some(FetchEvent::FetchError {
                        peer_id,
                        error: RequestError::ChannelClosed,
                    }))
                }
            }
        }

        Poll::Pending
    }
}

impl Default for TransactionFetcher {
    fn default() -> Self {
        Self {
            active_peers: LruMap::new(MAX_CONCURRENT_TX_REQUESTS),
            inflight_requests: Default::default(),
            buffered_hashes: LruCache::new(
                NonZeroUsize::new(MAX_CAPACITY_BUFFERED_HASHES)
                    .expect("buffered cache limit should be non-zero"),
            ),
            unknown_hashes: Default::default(),
        }
    }
}

/// Represents possible events from fetching transactions.
#[derive(Debug)]
enum FetchEvent {
    /// Triggered when transactions are successfully fetched.
    TransactionsFetched {
        /// The ID of the peer from which transactions were fetched.
        peer_id: PeerId,
        /// The transactions that were fetched, if available.
        transactions: Vec<PooledTransactionsElement>,
    },
    /// Triggered when there is an error in fetching transactions.
    FetchError {
        /// The ID of the peer from which an attempt to fetch transactions resulted in an error.
        peer_id: PeerId,
        /// The specific error that occurred while fetching.
        error: RequestError,
    },
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
}

// todo: standard for whole codebase
fn write_abbrev_hex(w: &mut String, hash: &[u8]) {
    let hex = hex::encode(hash);
    write!(w, "0x{}..{}, ", &hex[0..4], &hex[hex.len() - 4..])
        .expect("should write to abbreviated hex string")
}

fn abbrev_hex(hash: &[u8]) -> String {
    let mut w = String::new();
    write_abbrev_hex(&mut w, hash);
    w
}

fn abbrev_hex_hash_list(list: &[TxHash]) -> String {
    let mut w = String::new();
    write!(&mut w, "[").expect("should write to abbreviated hex list string");
    for item in list {
        write_abbrev_hex(&mut w, item.as_ref())
    }
    write!(&mut w, "]").expect("should write to abbreviated hex list string");
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::Testnet, NetworkConfigBuilder, NetworkManager};
    use alloy_rlp::Decodable;
    use reth_interfaces::sync::{NetworkSyncUpdater, SyncState};
    use reth_network_api::NetworkInfo;
    use reth_primitives::hex;
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::test_utils::{testing_pool, MockTransaction};
    use secp256k1::SecretKey;
    use std::{future::poll_fn, hash};

    async fn new_tx_manager() -> TransactionsManager<impl TransactionPool> {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let client = NoopProvider::default();

        let config = NetworkConfigBuilder::new(secret_key).disable_discovery().build(client);

        let pool = testing_pool();

        let (_network_handle, _network, transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
            .split_with_handle();

        transactions
    }

    fn default_cache<T: hash::Hash + Eq>() -> LruCache<T> {
        let limit = NonZeroUsize::new(MAX_ALTERNATIVE_PEERS_PER_TX.into()).unwrap();
        LruCache::new(limit)
    }

    // Returns (peer, channel-to-send-get-pooled-tx-response-on).
    fn new_mock_session(
        peer_id: PeerId,
        version: EthVersion,
    ) -> (Peer, mpsc::Receiver<PeerRequest>) {
        let (to_mock_session_tx, to_mock_session_rx) = mpsc::channel(1);

        (
            Peer {
                transactions: default_cache(),
                request_tx: PeerRequestSender::new(peer_id, to_mock_session_tx),
                //_inflight_requests_semaphore_rx,
                version,
                client_version: Arc::from(""),
            },
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
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
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
                    inflight_requests_semaphore_rx,
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
                        inflight_requests_semaphore_rx,
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
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
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
                    inflight_requests_semaphore_rx,
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
                        inflight_requests_semaphore_rx,
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
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
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
                    inflight_requests_semaphore_rx,
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
                        inflight_requests_semaphore_rx,
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
        let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
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
                    inflight_requests_semaphore_rx,
                    status,
                    version,
                } => transactions.on_network_event(NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    messages,
                    inflight_requests_semaphore_rx,
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
    async fn max_retries_tx_request() {
        reth_tracing::init_test_tracing();

        let mut tx_manager = new_tx_manager().await;
        let tx_fetcher = &mut tx_manager.transaction_fetcher;

        let peer_id_1 = PeerId::new([1; 64]);
        let peer_id_2 = PeerId::new([2; 64]);
        let eth_version = EthVersion::Eth66;
        let seen_hashes = [B256::from_slice(&[1; 32]), B256::from_slice(&[2; 32])];

        let (peer_1, mut to_mock_session_rx) = new_mock_session(peer_id_1, eth_version);
        tx_manager.peers.insert(peer_id_1, peer_1);

        // hashes are seen and currently not inflight, with one fallback peer, and are buffered
        // for first retry.
        let retries = 1;
        let mut backups = default_cache();
        backups.insert(peer_id_1);
        tx_fetcher.unknown_hashes.insert(seen_hashes[0], (retries, backups.clone()));
        tx_fetcher.unknown_hashes.insert(seen_hashes[1], (retries, backups));
        tx_fetcher.buffered_hashes.insert(seen_hashes[0]);
        tx_fetcher.buffered_hashes.insert(seen_hashes[1]);

        // peer_1 is idle
        assert!(tx_fetcher.is_idle(peer_id_1));

        // sends request for buffered hashes to peer_1
        tx_manager.request_buffered_hashes();

        let tx_fetcher = &mut tx_manager.transaction_fetcher;

        assert!(tx_fetcher.buffered_hashes.is_empty());
        // as long as request is in inflight peer_1 is not idle
        assert!(!tx_fetcher.is_idle(peer_id_1));

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
        assert!(tx_fetcher.is_idle(peer_id));
        // failing peer_1's request buffers requested hashes for retry
        assert_eq!(tx_fetcher.buffered_hashes.len(), 2);

        let (peer_2, mut to_mock_session_rx) = new_mock_session(peer_id_2, eth_version);
        tx_manager.peers.insert(peer_id_2, peer_2);

        // peer_2 announces same hashes as peer_1
        let msg =
            NewPooledTransactionHashes::Eth66(NewPooledTransactionHashes66(seen_hashes.to_vec()));
        tx_manager.on_new_pooled_transaction_hashes(peer_id_2, msg);

        let tx_fetcher = &mut tx_manager.transaction_fetcher;

        // since hashes are already seen, no changes to length of unknown hashes
        assert_eq!(tx_fetcher.unknown_hashes.len(), 2);
        // but hashes are taken out of buffer and packed into request to peer_2
        assert!(tx_fetcher.buffered_hashes.is_empty());

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

        // `MAX_REQUEST_RETRIES_PER_TX_HASH`, 2, for hashes reached however this time won't be
        // buffered for retry
        assert!(tx_fetcher.buffered_hashes.is_empty());
    }
}
