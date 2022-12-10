//! Listeners for the transaction-pool

use crate::{pool::events::TransactionEvent, traits::PropagateKind};
use reth_primitives::{rpc::TxHash, H256};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc::UnboundedSender;

type EventBroadcast = UnboundedSender<TransactionEvent>;

/// A type that broadcasts [`TransactionEvent`] to installed listeners.
///
/// This is essentially a multi-producer, multi-consumer channel where each event is broadcasted to
/// all active receivers.
#[derive(Debug, Default)]
pub(crate) struct PoolEventBroadcast {
    /// All listeners for certain transaction events.
    broadcasters: HashMap<TxHash, PoolEventBroadcaster>,
}

impl PoolEventBroadcast {
    /// Calls the broadcast callback with the `PoolEventBroadcaster` that belongs to the hash.
    fn broadcast_with<F>(&mut self, hash: &TxHash, callback: F)
    where
        F: FnOnce(&mut PoolEventBroadcaster),
    {
        let is_done = if let Some(sink) = self.broadcasters.get_mut(hash) {
            callback(sink);
            sink.is_done()
        } else {
            false
        };

        if is_done {
            self.broadcasters.remove(hash);
        }
    }

    /// Notify listeners about a transaction that was added to the pending queue.
    pub(crate) fn pending(&mut self, tx: &TxHash, replaced: Option<&TxHash>) {
        self.broadcast_with(tx, |notifier| notifier.pending());

        if let Some(replaced) = replaced {
            // notify listeners that this transaction was replaced
            self.broadcast_with(replaced, |notifier| notifier.replaced(*tx));
        }
    }

    /// Notify listeners about a transaction that was added to the queued pool.
    pub(crate) fn queued(&mut self, tx: &TxHash) {
        self.broadcast_with(tx, |notifier| notifier.queued());
    }

    /// Notify listeners about a transaction that was propagated.
    pub(crate) fn propagated(&mut self, tx: &TxHash, peers: Vec<PropagateKind>) {
        self.broadcast_with(tx, |notifier| notifier.propagated(peers));
    }

    /// Notify listeners about a transaction that was discarded.
    pub(crate) fn discarded(&mut self, tx: &TxHash) {
        self.broadcast_with(tx, |notifier| notifier.discarded());
    }

    /// Notify listeners that the transaction was mined
    pub(crate) fn mined(&mut self, tx: &TxHash, block_hash: H256) {
        self.broadcast_with(tx, |notifier| notifier.mined(block_hash));
    }
}

/// All Sender half(s) of the event channels for a specific transaction.
///
/// This mimics [tokio::sync::broadcast] but uses separate channels.
#[derive(Debug)]
struct PoolEventBroadcaster {
    /// Tracks whether the transaction this notifier can stop because the transaction was
    /// completed, or removed.
    is_done: bool,
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<EventBroadcast>,
}

impl PoolEventBroadcaster {
    fn broadcast(&mut self, event: TransactionEvent) {
        self.senders.retain(|sender| sender.send(event.clone()).is_ok())
    }

    fn is_done(&self) -> bool {
        self.senders.is_empty() || self.is_done
    }

    /// Transaction was moved to the pending queue.
    fn pending(&mut self) {
        self.broadcast(TransactionEvent::Pending)
    }

    /// Transaction was moved to the queued pool
    fn queued(&mut self) {
        self.broadcast(TransactionEvent::Queued)
    }

    /// Transaction was replaced with the given transaction
    fn replaced(&mut self, hash: TxHash) {
        self.broadcast(TransactionEvent::Replaced(hash));
        self.is_done = true;
    }

    /// Transaction was mined.
    fn mined(&mut self, block_hash: H256) {
        self.broadcast(TransactionEvent::Mined(block_hash));
        self.is_done = true;
    }

    /// Transaction was propagated.
    fn propagated(&mut self, peers: Vec<PropagateKind>) {
        self.broadcast(TransactionEvent::Propagated(Arc::new(peers)));
    }

    /// Transaction was replaced with the given transaction
    fn discarded(&mut self) {
        self.broadcast(TransactionEvent::Discarded);
        self.is_done = true;
    }
}
