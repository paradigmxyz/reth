//! Transaction management for the p2p network.

use crate::{
    cache::LruCache,
    manager::NetworkEvent,
    message::{PeerRequest, PeerRequestSender},
    network::NetworkHandleMessage,
    NetworkHandle,
};
use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use reth_eth_wire::{
    GetPooledTransactions, NewPooledTransactionHashes, PooledTransactions, Transactions,
};
use reth_interfaces::p2p::error::RequestResult;
use reth_primitives::{
    FromRecoveredTransaction, IntoRecoveredTransaction, PeerId, TransactionSigned, TxHash, H256,
};
use reth_transaction_pool::{error::PoolResult, TransactionPool};
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

/// Cache limit of transactions to keep track of for a single peer.
const PEER_TRANSACTION_CACHE_LIMIT: usize = 1024 * 10;

/// The future for inserting a function into the pool
pub type PoolImportFuture = Pin<Box<dyn Future<Output = PoolResult<TxHash>> + Send + 'static>>;

/// Api to interact with [`TransactionsManager`] task.
pub struct TransactionsHandle {
    /// Command channel to the [`TransactionsManager`]
    manager_tx: mpsc::UnboundedSender<TransactionsCommand>,
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
    /// Incoming events from the [`NetworkManager`]
    transaction_events: UnboundedReceiverStream<NetworkTransactionEvent>,
}

// === impl TransactionsManager ===

impl<Pool> TransactionsManager<Pool>
where
    Pool: TransactionPool + Clone,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    /// Sets up a new instance.
    ///
    /// Note: This expects an existing [`NetworkManager`] instance.
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
            network_events: UnboundedReceiverStream::new(network_events),
            inflight_requests: Default::default(),
            transactions_by_peers: Default::default(),
            pool_imports: Default::default(),
            peers: Default::default(),
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            pending_transactions: ReceiverStream::new(pending),
            transaction_events: UnboundedReceiverStream::new(from_network),
        }
    }

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

    /// Request handler for an incoming `NewPooledTransactionHashes`
    fn on_new_pooled_transactions(
        &mut self,
        peer_id: PeerId,
        msg: Arc<NewPooledTransactionHashes>,
    ) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            let mut transactions = Arc::try_unwrap(msg).unwrap_or_else(|arc| (*arc).clone()).0;

            // keep track of the transactions the peer knows
            peer.transactions.extend(transactions.clone());

            self.pool.retain_unknown(&mut transactions);

            if transactions.is_empty() {
                // nothing to request
                return
            }

            // request the missing transactions
            let (response, rx) = oneshot::channel();
            let req = PeerRequest::GetPooledTransactions {
                request: GetPooledTransactions(transactions),
                response,
            };

            if peer.request_tx.try_send(req).is_ok() {
                self.inflight_requests.push(GetPooledTxRequest { peer_id, response: rx })
            }
        }
    }

    /// Handles dedicated transaction events related tot the `eth` protocol.
    fn on_network_tx_event(&mut self, event: NetworkTransactionEvent) {
        match event {
            NetworkTransactionEvent::IncomingTransactions { peer_id, msg } => {
                let transactions = Arc::try_unwrap(msg).unwrap_or_else(|arc| (*arc).clone());
                self.import_transactions(peer_id, transactions.0);
            }
            NetworkTransactionEvent::IncomingPooledTransactionHashes { peer_id, msg } => {
                self.on_new_pooled_transactions(peer_id, msg)
            }
            NetworkTransactionEvent::GetPooledTransactions { peer_id, request, response } => {
                self.on_get_pooled_transactions(peer_id, request, response)
            }
        }
    }

    /// Handles a received event related to common network events.
    fn on_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::SessionClosed { peer_id } => {
                // remove the peer
                self.peers.remove(&peer_id);
            }
            NetworkEvent::SessionEstablished { peer_id, messages, .. } => {
                // insert a new peer
                self.peers.insert(
                    peer_id,
                    Peer {
                        transactions: LruCache::new(
                            NonZeroUsize::new(PEER_TRANSACTION_CACHE_LIMIT).unwrap(),
                        ),
                        request_tx: messages,
                    },
                );

                // Send a `NewPooledTransactionHashes` to the peer with _all_ transactions in the
                // pool
                let msg = Arc::new(NewPooledTransactionHashes(self.pool.pooled_transactions()));
                self.network.send_message(NetworkHandleMessage::SendPooledTransactionHashes {
                    peer_id,
                    msg,
                })
            }
        }
    }

    /// Starts the import process for the given transactions.
    fn import_transactions(&mut self, peer_id: PeerId, transactions: Vec<TransactionSigned>) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            for tx in transactions {
                // recover transaction
                let tx = if let Some(tx) = tx.into_ecrecovered() {
                    tx
                } else {
                    // TODO: report peer?
                    continue
                };

                // track that the peer knows this transaction
                peer.transactions.insert(tx.hash);

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
    }

    fn on_good_import(&mut self, hash: TxHash) {
        if let Some(_peers) = self.transactions_by_peers.remove(&hash) {
            // TODO report good peer?
        }
    }

    fn on_bad_import(&mut self, hash: TxHash) {
        if let Some(_peers) = self.transactions_by_peers.remove(&hash) {
            // TODO report bad peer?
        }
    }
}

/// An endless future.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<Pool> Future for TransactionsManager<Pool>
where
    Pool: TransactionPool + Clone + Unpin,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // drain incoming events
        while let Poll::Ready(Some(event)) = this.transaction_events.poll_next_unpin(cx) {
            this.on_network_tx_event(event);
        }

        while let Poll::Ready(Some(event)) = this.network_events.poll_next_unpin(cx) {
            this.on_network_event(event);
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

        // handle new transactions
        while let Poll::Ready(Some(_hash)) = this.pending_transactions.poll_next_unpin(cx) {
            // TODO(mattsse): propagate new transactions
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
                    this.import_transactions(req.peer_id, txs.0);
                }
                Poll::Ready(Ok(Err(_))) => {
                    // TODO report bad peer
                }
                Poll::Ready(Err(_)) => {
                    // TODO report bad peer
                }
            }
        }

        Poll::Pending
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
}

/// Commands to send to the [`TransactionManager`]
enum TransactionsCommand {
    Propagate(H256),
}

/// All events related to transactions emitted by the network
#[derive(Debug)]
pub enum NetworkTransactionEvent {
    /// Received list of transactions to the given peer.
    IncomingTransactions { peer_id: PeerId, msg: Arc<Transactions> },
    /// Received list of transactions hashes to the given peer.
    IncomingPooledTransactionHashes { peer_id: PeerId, msg: Arc<NewPooledTransactionHashes> },
    /// Incoming `GetPooledTransactions` request from a peer.
    GetPooledTransactions {
        peer_id: PeerId,
        request: GetPooledTransactions,
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    },
}
