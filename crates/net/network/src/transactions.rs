//! Transaction management for the p2p network.

use crate::{manager::NetworkEvent, NetworkHandle};
use reth_primitives::{Transaction, H256};
use reth_transaction_pool::TransactionPool;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

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
    /// All currently pending transactions
    pending_transactions: (),
    /// All the peers that have sent the same transactions.
    peers: HashMap<H256, Vec<()>>,
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
            pending_transactions: (),
            peers: Default::default(),
            command_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
        }
    }

    /// Returns a new handle that can send commands to this type.
    pub fn handle(&self) -> TransactionsHandle {
        TransactionsHandle { manager_tx: self.command_tx.clone() }
    }

    /// Executes an endless future
    pub async fn run(self) {}
}

/// Commands to send to the [`TransactionManager`]
enum TransactionsCommand {
    Propagate(H256),
}
