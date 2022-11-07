//! Listeners for the transaction-pool

use crate::pool::events::TransactionEvent;
use futures::channel::mpsc::UnboundedSender;
use reth_primitives::H256;
use std::{collections::HashMap, hash};

type EventSink<Hash> = UnboundedSender<TransactionEvent<Hash>>;

/// Transaction pool event listeners.
pub(crate) struct PoolEventListener<Hash: hash::Hash + Eq> {
    /// All listeners for certain transaction events.
    listeners: HashMap<Hash, PoolEventNotifier<Hash>>,
}

impl<Hash: hash::Hash + Eq + Clone> PoolEventListener<Hash> {
    /// Calls the notification callback with the `PoolEventListenerSender` that belongs to the hash.
    fn notify_with<F>(&mut self, hash: &Hash, callback: F)
    where
        F: FnOnce(&mut PoolEventNotifier<Hash>),
    {
        let is_done = if let Some(sink) = self.listeners.get_mut(hash) {
            callback(sink);
            sink.is_done()
        } else {
            false
        };

        if is_done {
            self.listeners.remove(hash);
        }
    }

    /// Notify listeners about a transaction that was added to the ready queue.
    pub(crate) fn ready(&mut self, tx: &Hash, replaced: Option<&Hash>) {
        self.notify_with(tx, |notifier| notifier.ready());

        if let Some(replaced) = replaced {
            // notify listeners that this transaction was replaced
            self.notify_with(replaced, |notifier| notifier.replaced(tx.clone()));
        }
    }

    /// Notify listeners about a transaction that was added to the queued pool.
    pub(crate) fn queued(&mut self, tx: &Hash) {
        self.notify_with(tx, |notifier| notifier.queued());
    }

    /// Notify listeners about a transaction that was discarded.
    pub(crate) fn discarded(&mut self, tx: &Hash) {
        self.notify_with(tx, |notifier| notifier.discarded());
    }

    /// Notify listeners that the transaction was mined
    pub(crate) fn mined(&mut self, tx: &Hash, block_hash: H256) {
        self.notify_with(tx, |notifier| notifier.mined(block_hash));
    }
}

impl<Hash: hash::Hash + Eq> Default for PoolEventListener<Hash> {
    fn default() -> Self {
        Self { listeners: Default::default() }
    }
}

/// Sender half(s) of the event channels for a specific transaction
#[derive(Debug)]
struct PoolEventNotifier<Hash> {
    /// Tracks whether the transaction this notifier can stop because the transaction was
    /// completed, or removed.
    is_done: bool,
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<EventSink<Hash>>,
}

impl<Hash: Clone> PoolEventNotifier<Hash> {
    fn notify(&mut self, event: TransactionEvent<Hash>) {
        self.senders.retain(|sender| sender.unbounded_send(event.clone()).is_ok())
    }

    fn is_done(&self) -> bool {
        self.senders.is_empty() || self.is_done
    }

    /// Transaction became ready.
    fn ready(&mut self) {
        self.notify(TransactionEvent::Pending)
    }

    /// Transaction was moved to the queued pool
    fn queued(&mut self) {
        self.notify(TransactionEvent::Queued)
    }

    /// Transaction was replaced with the given transaction
    fn replaced(&mut self, hash: Hash) {
        self.notify(TransactionEvent::Replaced(hash));
        self.is_done = true;
    }

    /// Transaction was mined.
    fn mined(&mut self, block_hash: H256) {
        self.notify(TransactionEvent::Mined(block_hash));
        self.is_done = true;
    }

    /// Transaction was replaced with the given transaction
    fn discarded(&mut self) {
        self.notify(TransactionEvent::Discarded);
        self.is_done = true;
    }
}
