//! Transaction management for the p2p network.

use reth_primitives::H256;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Api to interact with [`TransactionsManager`] task.
pub struct TransactionsHandle {
    /// Command channel to the [`TransactionsManager`]
    manager_tx: mpsc::UnboundedSender<TransactionsCommand>,
}

/// Manages transactions on top of the p2p network.
///
/// This can be spawned to another task and is supposed to be run as background service while
/// [`TransactionsHandle`] is used as frontend to send commands to.
#[must_use = "Manager does nothing unless polled."]
pub struct TransactionsManager {
    /// Network access.
    network: (),
    /// All currently pending transactions
    pending_transactions: (),
    /// All the peers that have sent the same transactions.
    peers: HashMap<H256, Vec<()>>,
    /// Incoming commands from [`TransactionsHandle`].
    incoming: mpsc::UnboundedReceiver<TransactionsCommand>,
}

/// Commands to send to the [`TransactionManager`]
enum TransactionsCommand {
    Propagate(H256),
}
