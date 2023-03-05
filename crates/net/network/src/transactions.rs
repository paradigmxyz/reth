//! Transactions management for the p2p network.

use crate::{
    cache::LruCache,
    manager::NetworkEvent,
    message::{PeerRequest, PeerRequestSender},
    metrics::TransactionsManagerMetrics,
    NetworkHandle,
};
use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use reth_eth_wire::{
    EthVersion, GetPooledTransactions, NewPooledTransactionHashes, NewPooledTransactionHashes66,
    NewPooledTransactionHashes68, PooledTransactions, Transactions,
};
use reth_interfaces::{p2p::error::RequestResult, sync::SyncStateProvider};
use reth_network_api::{Peers, ReputationChangeKind};
use reth_primitives::{
    FromRecoveredTransaction, IntoRecoveredTransaction, PeerId, TransactionSigned, TxHash, H256,
};
use reth_rlp::Encodable;
use reth_transaction_pool::{
    error::PoolResult, PoolTransaction, PropagateKind, PropagatedTransactions, TransactionPool,
    ValidPoolTransaction,
};
use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};
use tracing::trace;

/// Cache limit of transactions to keep track of for a single peer.
const PEER_TRANSACTION_CACHE_LIMIT: usize = 1024 * 10;

/// Soft limit for NewPooledTransactions
const NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT: usize = 4096;

/// The future for inserting a function into the pool
pub type PoolImportFuture = Pin<Box<dyn Future<Output = PoolResult<TxHash>> + Send + 'static>>;

/// Api to interact with [`TransactionsManager`] task.
pub struct TransactionsHandle {
    /// Command channel to the [`TransactionsManager`]
    manager_tx: mpsc::UnboundedSender<TransactionsCommand>,
}

// === impl TransactionsHandle ===

impl TransactionsHandle {
    fn send(&self, cmd: TransactionsCommand) {
        let _ = self.manager_tx.send(cmd);
    }

    /// Manually propagate the transaction that belongs to the hash.
    pub fn propagate(&self, hash: TxHash) {
        self.send(TransactionsCommand::PropagateHash(hash))
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
    /// All currently active requests for pooled transactions.
    inflight_requests: Vec<GetPooledTxRequest>,
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
    transaction_events: UnboundedReceiverStream<NetworkTransactionEvent>,
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

        // install a listener for new transactions
        let pending = pool.pending_transactions_listener();

        Self {
            pool,
            network,
            network_events,
            inflight_requests: Default::default(),
            transactions_by_peers: Default::default(),
            pool_imports: Default::default(),
            peers: Default::default(),
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            pending_transactions: ReceiverStream::new(pending),
            transaction_events: UnboundedReceiverStream::new(from_network),
            metrics: Default::default(),
        }
    }
}

// === impl TransactionsManager ===

impl<Pool> TransactionsManager<Pool>
where
    Pool: TransactionPool + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    /// Returns a new handle that can send commands to this type.
    pub fn handle(&self) -> TransactionsHandle {
        TransactionsHandle { manager_tx: self.command_tx.clone() }
    }

    /// Request handler for an incoming request for transactions
    fn on_get_pooled_transactions(
        &mut self,
        peer_id: PeerId,
        request: GetPooledTransactions,
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    ) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            let transactions = self
                .pool
                .get_all(request.0)
                .into_iter()
                .map(|tx| tx.transaction.to_recovered_transaction().into_signed())
                .collect::<Vec<_>>();

            // we sent a response at which point we assume that the peer is aware of the transaction
            peer.transactions.extend(transactions.iter().map(|tx| tx.hash()));

            let resp = PooledTransactions(transactions);
            let _ = response.send(Ok(resp));
        }
    }

    /// Invoked when a new transaction is pending.
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
    fn on_new_transactions(&mut self, hashes: impl IntoIterator<Item = TxHash>) {
        // Nothing to propagate while syncing
        if self.network.is_syncing() {
            return
        }

        trace!(target: "net::tx", "Start propagating transactions");

        let propagated = self.propagate_transactions(
            self.pool
                .get_all(hashes)
                .into_iter()
                .map(|tx| {
                    let tx = Arc::new(tx.transaction.to_recovered_transaction().into_signed());
                    PropagateTransaction::new(tx)
                })
                .collect(),
        );

        // notify pool so events get fired
        self.pool.on_propagated(propagated);
    }

    /// Propagate the transactions to all connected peers either as full objects or hashes
    ///
    /// The message for new pooled hashes depends on the negotiated version of the stream.
    /// See [NewPooledTransactionHashes](NewPooledTransactionHashes)
    fn propagate_transactions(
        &mut self,
        to_propagate: Vec<PropagateTransaction>,
    ) -> PropagatedTransactions {
        let mut propagated = PropagatedTransactions::default();

        // send full transactions to a fraction fo the connected peers (square root of the total
        // number of connected peers)
        let max_num_full = (self.peers.len() as f64).sqrt() as usize + 1;

        // Note: Assuming ~random~ order due to random state of the peers map hasher
        for (idx, (peer_id, peer)) in self.peers.iter_mut().enumerate() {
            // filter all transactions unknown to the peer
            let mut hashes = PooledTransactionsHashesBuilder::new(peer.version);
            let mut full_transactions = Vec::new();
            for tx in to_propagate.iter() {
                if peer.transactions.insert(tx.hash()) {
                    hashes.push(tx);
                    full_transactions.push(Arc::clone(&tx.transaction));
                }
            }
            let hashes = hashes.build();

            if !full_transactions.is_empty() {
                if idx > max_num_full {
                    for hash in hashes.iter_hashes().copied() {
                        propagated.0.entry(hash).or_default().push(PropagateKind::Hash(*peer_id));
                    }
                    // send hashes of transactions
                    self.network.send_transactions_hashes(*peer_id, hashes);
                } else {
                    // send full transactions
                    self.network.send_transactions(*peer_id, full_transactions);

                    for hash in hashes.into_iter_hashes() {
                        propagated.0.entry(hash).or_default().push(PropagateKind::Full(*peer_id));
                    }
                }
            }
        }

        // Update propagated transactions metrics
        self.metrics.propagated_transactions.increment(propagated.0.len() as u64);

        propagated
    }

    /// Request handler for an incoming `NewPooledTransactionHashes`
    fn on_new_pooled_transaction_hashes(
        &mut self,
        peer_id: PeerId,
        msg: NewPooledTransactionHashes,
    ) {
        // If the node is currently syncing, ignore transactions
        if self.network.is_syncing() {
            return
        }

        let mut num_already_seen = 0;

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            let mut hashes = msg.into_hashes();
            // keep track of the transactions the peer knows
            for tx in hashes.iter().copied() {
                if !peer.transactions.insert(tx) {
                    num_already_seen += 1;
                }
            }

            self.pool.retain_unknown(&mut hashes);

            if hashes.is_empty() {
                // nothing to request
                return
            }

            // request the missing transactions
            let (response, rx) = oneshot::channel();
            let req = PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(hashes),
                response,
            };

            if peer.request_tx.try_send(req).is_ok() {
                self.inflight_requests.push(GetPooledTxRequest { peer_id, response: rx })
            }
        }

        if num_already_seen > 0 {
            self.report_bad_message(peer_id);
        }
    }

    /// Handles dedicated transaction events related tot the `eth` protocol.
    fn on_network_tx_event(&mut self, event: NetworkTransactionEvent) {
        match event {
            NetworkTransactionEvent::IncomingTransactions { peer_id, msg } => {
                self.import_transactions(peer_id, msg.0, TransactionSource::Broadcast);
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
            TransactionsCommand::PropagateHash(hash) => {
                self.on_new_transactions(std::iter::once(hash))
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
            NetworkEvent::SessionEstablished { peer_id, messages, version, .. } => {
                // insert a new peer into the peerset
                self.peers.insert(
                    peer_id,
                    Peer {
                        transactions: LruCache::new(
                            NonZeroUsize::new(PEER_TRANSACTION_CACHE_LIMIT).unwrap(),
                        ),
                        request_tx: messages,
                        version,
                    },
                );

                // Send a `NewPooledTransactionHashes` to the peer with up to
                // `NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT` transactions in the
                // pool
                if !self.network.is_syncing() {
                    let peer = self.peers.get_mut(&peer_id).expect("is present; qed");

                    let mut msg_builder = PooledTransactionsHashesBuilder::new(version);

                    for pooled_tx in self
                        .pool
                        .pooled_transactions()
                        .into_iter()
                        .take(NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT)
                    {
                        peer.transactions.insert(*pooled_tx.hash());
                        msg_builder.push_pooled(pooled_tx);
                    }

                    let msg = msg_builder.build();
                    self.network.send_transactions_hashes(peer_id, msg);
                }
            }
            // TODO Add remaining events
            _ => {}
        }
    }

    /// Starts the import process for the given transactions.
    fn import_transactions(
        &mut self,
        peer_id: PeerId,
        transactions: Vec<TransactionSigned>,
        source: TransactionSource,
    ) {
        // If the node is currently syncing, ignore transactions
        if self.network.is_syncing() {
            return
        }

        // tracks the quality of the given transactions
        let mut has_bad_transactions = false;
        let mut num_already_seen = 0;

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            for tx in transactions {
                // recover transaction
                let tx = if let Some(tx) = tx.into_ecrecovered() {
                    tx
                } else {
                    has_bad_transactions = true;
                    continue
                };

                // track that the peer knows this transaction, but only if this is a new broadcast.
                // If we received the transactions as the response to our GetPooledTransactions
                // requests (based on received `NewPooledTransactionHashes`) then we already
                // recorded the hashes in [`Self::on_new_pooled_transaction_hashes`]
                if source.is_broadcast() && !peer.transactions.insert(tx.hash) {
                    num_already_seen += 1;
                }

                match self.transactions_by_peers.entry(tx.hash) {
                    Entry::Occupied(mut entry) => {
                        // transaction was already inserted
                        entry.get_mut().push(peer_id);
                    }
                    Entry::Vacant(entry) => {
                        // this is a new transaction that should be imported into the pool
                        let pool_transaction = <Pool::Transaction as FromRecoveredTransaction>::from_recovered_transaction(tx);

                        let pool = self.pool.clone();
                        let import = Box::pin(async move {
                            pool.add_external_transaction(pool_transaction).await
                        });

                        self.pool_imports.push(import);
                        entry.insert(vec![peer_id]);
                    }
                }
            }
        }

        if has_bad_transactions || num_already_seen > 0 {
            self.report_bad_message(peer_id);
        }
    }

    fn report_bad_message(&self, peer_id: PeerId) {
        self.network.reputation_change(peer_id, ReputationChangeKind::BadTransactions);
    }

    fn on_good_import(&mut self, hash: TxHash) {
        self.transactions_by_peers.remove(&hash);
    }

    fn on_bad_import(&mut self, hash: TxHash) {
        if let Some(peers) = self.transactions_by_peers.remove(&hash) {
            for peer_id in peers {
                self.report_bad_message(peer_id);
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
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
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

        // Advance all requests.
        // We remove each request one by one and add them back.
        for idx in (0..this.inflight_requests.len()).rev() {
            let mut req = this.inflight_requests.swap_remove(idx);
            match req.response.poll_unpin(cx) {
                Poll::Pending => {
                    this.inflight_requests.push(req);
                }
                Poll::Ready(Ok(Ok(txs))) => {
                    this.import_transactions(req.peer_id, txs.0, TransactionSource::Response);
                }
                Poll::Ready(Ok(Err(_))) => {
                    this.report_bad_message(req.peer_id);
                }
                Poll::Ready(Err(_)) => {
                    this.report_bad_message(req.peer_id);
                }
            }
        }

        // Advance all imports
        while let Poll::Ready(Some(import_res)) = this.pool_imports.poll_next_unpin(cx) {
            match import_res {
                Ok(hash) => {
                    this.on_good_import(hash);
                }
                Err(err) => {
                    this.on_bad_import(*err.hash());
                }
            }
        }

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
    tx_type: u8,
    length: usize,
    transaction: Arc<TransactionSigned>,
}

// === impl PropagateTransaction ===

impl PropagateTransaction {
    fn hash(&self) -> TxHash {
        self.transaction.hash
    }

    fn new(transaction: Arc<TransactionSigned>) -> Self {
        Self { tx_type: transaction.tx_type().into(), length: transaction.length(), transaction }
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
                msg.sizes.push(pooled_tx.encoded_length);
                msg.types.push(pooled_tx.transaction.tx_type());
            }
        }
    }

    fn push(&mut self, tx: &PropagateTransaction) {
        match self {
            PooledTransactionsHashesBuilder::Eth66(msg) => msg.0.push(tx.hash()),
            PooledTransactionsHashesBuilder::Eth68(msg) => {
                msg.hashes.push(tx.hash());
                msg.sizes.push(tx.length);
                msg.types.push(tx.tx_type);
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
#[allow(missing_docs)]
struct GetPooledTxRequest {
    peer_id: PeerId,
    response: oneshot::Receiver<RequestResult<PooledTransactions>>,
}

/// Tracks a single peer
struct Peer {
    /// Keeps track of transactions that we know the peer has seen.
    transactions: LruCache<H256>,
    /// A communication channel directly to the session task.
    request_tx: PeerRequestSender,
    /// negotiated version of the session.
    version: EthVersion,
}

/// Commands to send to the [`TransactionsManager`](crate::transactions::TransactionsManager)
enum TransactionsCommand {
    PropagateHash(H256),
}

/// All events related to transactions emitted by the network.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum NetworkTransactionEvent {
    /// Received list of transactions from the given peer.
    IncomingTransactions { peer_id: PeerId, msg: Transactions },
    /// Received list of transactions hashes to the given peer.
    IncomingPooledTransactionHashes { peer_id: PeerId, msg: NewPooledTransactionHashes },
    /// Incoming `GetPooledTransactions` request from a peer.
    GetPooledTransactions {
        peer_id: PeerId,
        request: GetPooledTransactions,
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NetworkConfigBuilder, NetworkManager};
    use reth_interfaces::sync::{SyncState, SyncStateUpdater};
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::test_utils::testing_pool;
    use secp256k1::SecretKey;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_ignored_tx_broadcasts_while_syncing() {
        reth_tracing::init_test_tracing();

        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let client = NoopProvider::default();
        let pool = testing_pool();
        let config = NetworkConfigBuilder::new(secret_key).build(client);
        let (handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
            .split_with_handle();

        tokio::task::spawn(network);

        handle.update_sync_state(SyncState::Downloading { target_block: 100 });
        assert!(handle.is_syncing());

        let peer_id = PeerId::random();

        transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
            peer_id,
            msg: Transactions(vec![TransactionSigned::default()]),
        });

        assert!(pool.is_empty());
    }
}
