//! Transactions management for the p2p network.

use crate::{
    cache::LruCache,
    manager::NetworkEvent,
    message::{PeerRequest, PeerRequestSender},
    metrics::{TransactionsManagerMetrics, NETWORK_POOL_TRANSACTIONS_SCOPE},
    NetworkHandle,
};
use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use reth_eth_wire::{
    EthVersion, GetPooledTransactions, NewPooledTransactionHashes, NewPooledTransactionHashes66,
    NewPooledTransactionHashes68, PooledTransactions, Transactions,
};
use reth_interfaces::{p2p::error::RequestResult, sync::SyncStateProvider};
use reth_metrics::common::mpsc::UnboundedMeteredReceiver;
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
use tracing::{debug, trace};

/// Cache limit of transactions to keep track of for a single peer.
const PEER_TRANSACTION_CACHE_LIMIT: usize = 1024 * 10;

/// Soft limit for NewPooledTransactions
const NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT: usize = 4096;

/// The target size for the message of full transactions.
const MAX_FULL_TRANSACTIONS_PACKET_SIZE: usize = 100 * 1024;

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
    Pool: TransactionPool + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    /// Returns a new handle that can send commands to this type.
    pub fn handle(&self) -> TransactionsHandle {
        TransactionsHandle { manager_tx: self.command_tx.clone() }
    }

    fn update_import_metrics(&self) {
        self.metrics.pending_pool_imports.set(self.pool_imports.len() as f64);
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
        for (peer_idx, (peer_id, peer)) in self.peers.iter_mut().enumerate() {
            // filter all transactions unknown to the peer
            let mut hashes = PooledTransactionsHashesBuilder::new(peer.version);
            let mut full_transactions = FullTransactionsBuilder::default();

            for tx in to_propagate.iter() {
                if peer.transactions.insert(tx.hash()) {
                    hashes.push(tx);
                    full_transactions.push(tx);
                }
            }
            let mut new_pooled_hashes = hashes.build();

            if !new_pooled_hashes.is_empty() {
                // determine whether to send full tx objects or hashes.
                if peer_idx > max_num_full {
                    // enforce tx soft limit per message for the (unlikely) event the number of
                    // hashes exceeds it
                    new_pooled_hashes.truncate(NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT);

                    for hash in new_pooled_hashes.iter_hashes().copied() {
                        propagated.0.entry(hash).or_default().push(PropagateKind::Hash(*peer_id));
                    }
                    // send hashes of transactions
                    self.network.send_transactions_hashes(*peer_id, new_pooled_hashes);
                } else {
                    // send full transactions
                    self.network.send_transactions(*peer_id, full_transactions.build());

                    for hash in new_pooled_hashes.into_iter_hashes() {
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

            if num_already_seen > 0 {
                self.metrics.messages_with_already_seen_hashes.increment(1);
                debug!(target: "net::tx", num_hashes=%num_already_seen, ?peer_id, client=?peer.client_version, "Peer sent already seen hashes");
            }
        }

        if num_already_seen > 0 {
            self.report_already_seen(peer_id);
        }
    }

    /// Handles dedicated transaction events related to the `eth` protocol.
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
            NetworkEvent::SessionEstablished {
                peer_id, client_version, messages, version, ..
            } => {
                // insert a new peer into the peerset
                self.peers.insert(
                    peer_id,
                    Peer {
                        transactions: LruCache::new(
                            NonZeroUsize::new(PEER_TRANSACTION_CACHE_LIMIT).unwrap(),
                        ),
                        request_tx: messages,
                        version,
                        client_version,
                    },
                );

                // Send a `NewPooledTransactionHashes` to the peer with up to
                // `NEW_POOLED_TRANSACTION_HASHES_SOFT_LIMIT` transactions in the
                // pool
                if !self.network.is_syncing() {
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
                if source.is_broadcast() && !peer.transactions.insert(tx.hash()) {
                    num_already_seen += 1;
                }

                match self.transactions_by_peers.entry(tx.hash()) {
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

            if num_already_seen > 0 {
                self.metrics.messages_with_already_seen_transactions.increment(1);
                debug!(target: "net::tx", num_txs=%num_already_seen, ?peer_id, client=?peer.client_version, "Peer sent already seen transactions");
            }
        }

        if has_bad_transactions || num_already_seen > 0 {
            self.report_already_seen(peer_id);
        }
    }

    fn report_bad_message(&self, peer_id: PeerId) {
        trace!(target: "net::tx", ?peer_id, "Penalizing peer for bad transaction");
        self.metrics.reported_bad_transactions.increment(1);
        self.network.reputation_change(peer_id, ReputationChangeKind::BadTransactions);
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

        this.update_import_metrics();

        // Advance all imports
        while let Poll::Ready(Some(import_res)) = this.pool_imports.poll_next_unpin(cx) {
            match import_res {
                Ok(hash) => {
                    this.on_good_import(hash);
                }
                Err(err) => {
                    // if we're syncing and the transaction is bad we ignore it, otherwise we
                    // penalize the peer that sent the bad transaction with the assumption that the
                    // peer should have known that this transaction is bad. (e.g. consensus rules)
                    if err.is_bad_transaction() && !this.network.is_syncing() {
                        trace!(target: "net::tx", ?err, "Bad transaction import");
                        this.on_bad_import(*err.hash());
                        continue
                    }
                    this.on_good_import(*err.hash());
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
    tx_type: u8,
    size: usize,
    transaction: Arc<TransactionSigned>,
}

// === impl PropagateTransaction ===

impl PropagateTransaction {
    fn hash(&self) -> TxHash {
        self.transaction.hash()
    }

    fn new(transaction: Arc<TransactionSigned>) -> Self {
        Self { tx_type: transaction.tx_type().into(), size: transaction.length(), transaction }
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
                msg.sizes.push(tx.size);
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
    /// The peer's client version.
    #[allow(unused)]
    client_version: Arc<String>,
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
    use crate::{test_utils::Testnet, NetworkConfigBuilder, NetworkManager};
    use reth_interfaces::sync::{NetworkSyncUpdater, SyncState};
    use reth_network_api::NetworkInfo;
    use reth_provider::test_utils::NoopProvider;
    use reth_rlp::Decodable;
    use reth_transaction_pool::test_utils::{testing_pool, MockTransaction};
    use secp256k1::SecretKey;

    #[tokio::test(flavor = "multi_thread")]
    #[cfg_attr(not(feature = "geth-tests"), ignore)]
    async fn test_ignored_tx_broadcasts_while_syncing() {
        reth_tracing::init_test_tracing();

        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let client = NoopProvider::default();
        let pool = testing_pool();
        let config = NetworkConfigBuilder::new(secret_key)
            .disable_discovery()
            .listener_port(0)
            .build(client);
        let (handle, network, mut transactions, _) = NetworkManager::new(config)
            .await
            .unwrap()
            .into_builder()
            .transactions(pool.clone())
            .split_with_handle();

        tokio::task::spawn(network);

        handle.update_sync_state(SyncState::Syncing);
        assert!(NetworkInfo::is_syncing(&handle));

        let peer_id = PeerId::random();

        transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
            peer_id,
            msg: Transactions(vec![TransactionSigned::default()]),
        });

        assert!(pool.is_empty());
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
        let input = hex::decode("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76").unwrap();
        let signed_tx = TransactionSigned::decode(&mut &input[..]).unwrap();
        transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
            peer_id: *handle1.peer_id(),
            msg: Transactions(vec![signed_tx.clone()]),
        });
        assert_eq!(
            *handle1.peer_id(),
            transactions.transactions_by_peers.get(&signed_tx.hash()).unwrap()[0]
        );
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
}
