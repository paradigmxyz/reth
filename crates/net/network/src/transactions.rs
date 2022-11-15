//! Transaction management for the p2p network.

use crate::{cache::LruCache, manager::NetworkEvent, message::PeerRequestSender, NetworkHandle};
use futures::stream::FuturesUnordered;
use reth_primitives::{PeerId, Transaction, H256};
use reth_transaction_pool::TransactionPool;
use std::{
    collections::{hash_map::Entry, HashMap},
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Cache limit of transactions to keep track of for a single peer.
const PEER_TRANSACTION_CACHE_LIMIT: usize = 1024;

/// The future for inserting a function into the pool
pub type PoolImportFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

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
    /// All currently pending transactions grouped by peers.
    ///
    /// This way we can track incoming transactions and prevent multiple pool imports for the same
    /// transaction
    transactions_by_peers: HashMap<H256, Vec<PeerId>>,
    /// Transactions that are currently imported into the `Pool`
    pool_imports: FuturesUnordered<PoolImportFuture>,
    /// All the connected peers.
    peers: HashMap<PeerId, Peer>,
    /// Send half for the command channel.
    command_tx: mpsc::UnboundedSender<TransactionsCommand>,
    /// Incoming commands from [`TransactionsHandle`].
    command_rx: UnboundedReceiverStream<TransactionsCommand>,
}

// === impl TransactionsManager ===

impl<Pool> TransactionsManager<Pool>
where
    Pool: TransactionPool<Transaction = Transaction>,
{
    /// Sets up a new instance.
    pub fn new(network: NetworkHandle, pool: Pool) -> Self {
        let network_events = network.event_listener();
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        Self {
            pool,
            network,
            network_events: UnboundedReceiverStream::new(network_events),
            transactions_by_peers: Default::default(),
            pool_imports: Default::default(),
            peers: Default::default(),
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
        }
    }

    /// Returns a new handle that can send commands to this type.
    pub fn handle(&self) -> TransactionsHandle {
        TransactionsHandle { manager_tx: self.command_tx.clone() }
    }

    /// Handles a received event
    async fn on_event(&mut self, event: NetworkEvent) {
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

                // TODO send `NewPooledTransactionHashes
            }
            NetworkEvent::IncomingTransactions { peer_id, msg } => {
                let transactions = Arc::try_unwrap(msg).unwrap_or_else(|arc| (*arc).clone());

                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    for tx in transactions.0 {
                        // track that the peer knows this transaction
                        peer.transactions.insert(tx.hash);

                        match self.transactions_by_peers.entry(tx.hash) {
                            Entry::Occupied(mut entry) => {
                                // transaction was already inserted
                                entry.get_mut().push(peer_id);
                            }
                            Entry::Vacant(_) => {
                                // TODO import into the pool
                            }
                        }
                    }
                }
            }
            NetworkEvent::IncomingPooledTransactionHashes { .. } => {}
            NetworkEvent::GetPooledTransactions { .. } => {}
        }
    }

    /// Executes an endless future
    pub async fn run(self) {}
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
