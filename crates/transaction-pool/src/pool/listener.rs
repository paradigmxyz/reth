//! Listeners for the transaction-pool

use crate::{
    pool::events::{PoolTransactionEvent, TransactionEvent},
    traits::PropagateKind,
};
use futures_util::Stream;
use reth_primitives::{TxHash, H256};
use std::{
    collections::{hash_map::Entry, HashMap},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{
    error::TrySendError, Receiver, Sender, UnboundedReceiver, UnboundedSender,
};

/// The size of the event channel used to propagate transaction events.
const TX_POOL_EVENT_CHANNEL_SIZE: usize = 1024;

/// A Stream that receives [TransactionEvent] only for the transaction with the given hash.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct TransactionEvents {
    hash: TxHash,
    events: UnboundedReceiver<PoolTransactionEvent>,
}

impl TransactionEvents {
    /// The hash for this transaction
    pub fn hash(&self) -> TxHash {
        self.hash
    }
}

impl Stream for TransactionEvents {
    type Item = PoolTransactionEvent;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().events.poll_recv(cx)
    }
}

/// A Stream that receives [PoolTransactionEvent] for _all_ transaction.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct AllTransactionsEvents {
    pub(crate) events: Receiver<PoolTransactionEvent>,
}

impl Stream for AllTransactionsEvents {
    type Item = PoolTransactionEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().events.poll_recv(cx)
    }
}

/// A type that broadcasts [`TransactionEvent`] to installed listeners.
///
/// This is essentially a multi-producer, multi-consumer channel where each event is broadcast to
/// all active receivers.
#[derive(Default, Debug)]
pub(crate) struct PoolEventBroadcast {
    /// All listeners for all transaction events.
    all_events_broadcaster: AllPoolEventsBroadcaster,
    /// All listeners for events for a certain transaction hash.
    broadcasters_by_hash: HashMap<TxHash, PoolEventBroadcaster>,
}

impl PoolEventBroadcast {
    /// Calls the broadcast callback with the `PoolEventBroadcaster` that belongs to the hash.
    fn broadcast_event(&mut self, hash: &TxHash, event: TransactionEvent) {
        // Broadcast to all listeners for the transaction hash.
        if let Entry::Occupied(mut sink) = self.broadcasters_by_hash.entry(*hash) {
            sink.get_mut().broadcast(*hash, event.clone());

            if sink.get().is_empty() || event.is_final() {
                sink.remove();
            }
        }

        // Broadcast to all listeners for all transactions.
        self.all_events_broadcaster.broadcast(*hash, event);
    }

    /// Create a new subscription for the given transaction hash.
    pub(crate) fn subscribe(&mut self, tx_hash: TxHash) -> TransactionEvents {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        match self.broadcasters_by_hash.entry(tx_hash) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().senders.push(tx);
            }
            Entry::Vacant(entry) => {
                entry.insert(PoolEventBroadcaster { senders: vec![tx] });
            }
        };
        TransactionEvents { hash: tx_hash, events: rx }
    }

    /// Create a new subscription for all transactions.
    pub(crate) fn subscribe_all(&mut self) -> AllTransactionsEvents {
        let (tx, rx) = tokio::sync::mpsc::channel(TX_POOL_EVENT_CHANNEL_SIZE);
        self.all_events_broadcaster.senders.push(tx);
        AllTransactionsEvents { events: rx }
    }

    /// Notify listeners about a transaction that was added to the pending queue.
    pub(crate) fn pending(&mut self, tx: &TxHash, replaced: Option<&TxHash>) {
        self.broadcast_event(tx, TransactionEvent::Pending);

        if let Some(replaced) = replaced {
            // notify listeners that this transaction was replaced
            self.broadcast_event(replaced, TransactionEvent::Replaced(*tx));
        }
    }

    /// Notify listeners about a transaction that was added to the queued pool.
    pub(crate) fn queued(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Queued);
    }

    /// Notify listeners about a transaction that was propagated.
    pub(crate) fn propagated(&mut self, tx: &TxHash, peers: Vec<PropagateKind>) {
        self.broadcast_event(tx, TransactionEvent::Propagated(Arc::new(peers)));
    }

    /// Notify listeners about a transaction that was discarded.
    pub(crate) fn discarded(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Discarded);
    }

    /// Notify listeners that the transaction was mined
    pub(crate) fn mined(&mut self, tx: &TxHash, block_hash: H256) {
        self.broadcast_event(tx, TransactionEvent::Mined(block_hash));
    }
}

/// All Sender half(s) of the event channels for all transactions.
///
/// This mimics [tokio::sync::broadcast] but uses separate channels.
#[derive(Default, Debug)]
struct AllPoolEventsBroadcaster {
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<Sender<PoolTransactionEvent>>,
}

impl AllPoolEventsBroadcaster {
    // Broadcast an event to all listeners. Dropped listeners are silently evicted.
    fn broadcast(&mut self, tx_hash: TxHash, event: TransactionEvent) {
        self.senders.retain(|sender| {
            match sender.try_send(PoolTransactionEvent::new(tx_hash, event.clone())) {
                Ok(_) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Closed(_)) => false,
            }
        })
    }
}

/// All Sender half(s) of the event channels for a specific transaction.
///
/// This mimics [tokio::sync::broadcast] but uses separate channels and is unbounded.
#[derive(Default, Debug)]
struct PoolEventBroadcaster {
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<UnboundedSender<PoolTransactionEvent>>,
}

impl PoolEventBroadcaster {
    /// Returns `true` if there are no more listeners remaining.
    fn is_empty(&self) -> bool {
        self.senders.is_empty()
    }

    // Broadcast an event to all listeners. Dropped listeners are silently evicted.
    fn broadcast(&mut self, tx_hash: TxHash, event: TransactionEvent) {
        self.senders
            .retain(|sender| sender.send(PoolTransactionEvent::new(tx_hash, event.clone())).is_ok())
    }
}
