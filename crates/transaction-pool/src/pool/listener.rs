//! Listeners for the transaction-pool

use crate::{pool::events::TransactionEvent, traits::PropagateKind};
use futures_util::Stream;
use reth_primitives::{TxHash, H256};
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// A Stream that receives [TransactionEvent] for the transaction with the given hash.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct TransactionEvents {
    hash: TxHash,
    events: UnboundedReceiver<TransactionEvent>,
}

impl TransactionEvents {
    /// The hash for this transaction
    pub fn hash(&self) -> TxHash {
        self.hash
    }
}

impl Stream for TransactionEvents {
    type Item = TransactionEvent;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().events.poll_recv(cx)
    }
}

type EventBroadcast = UnboundedSender<TransactionEvent>;

/// A type that broadcasts [`TransactionEvent`] to installed listeners.
///
/// This is essentially a multi-producer, multi-consumer channel where each event is broadcast to
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

    /// Create a new subscription for the given transaction hash.
    pub(crate) fn subscribe(&mut self, tx_hash: TxHash) -> TransactionEvents {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        match self.broadcasters.entry(tx_hash) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().senders.push(tx);
            }
            Entry::Vacant(entry) => {
                entry.insert(PoolEventBroadcaster { is_done: false, senders: vec![tx] });
            }
        };
        TransactionEvents { hash: tx_hash, events: rx }
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
