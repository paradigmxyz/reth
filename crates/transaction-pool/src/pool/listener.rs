//! Listeners for the transaction-pool

use crate::{
    pool::events::{FullTransactionEvent, NewTransactionEvent, TransactionEvent},
    traits::{NewBlobSidecar, PropagateKind},
    PoolTransaction, ValidPoolTransaction,
};
use alloy_primitives::{TxHash, B256};
use futures_util::Stream;
use std::{
    collections::{hash_map::Entry, HashMap},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{
    self as mpsc, error::TrySendError, Receiver, Sender, UnboundedReceiver, UnboundedSender,
};
use tracing::debug;
/// The size of the event channel used to propagate transaction events.
const TX_POOL_EVENT_CHANNEL_SIZE: usize = 1024;

/// A Stream that receives [`TransactionEvent`] only for the transaction with the given hash.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct TransactionEvents {
    hash: TxHash,
    events: UnboundedReceiver<TransactionEvent>,
}

impl TransactionEvents {
    /// The hash for this transaction
    pub const fn hash(&self) -> TxHash {
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

/// A Stream that receives [`FullTransactionEvent`] for _all_ transaction.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct AllTransactionsEvents<T: PoolTransaction> {
    pub(crate) events: Receiver<FullTransactionEvent<T>>,
}

impl<T: PoolTransaction> AllTransactionsEvents<T> {
    /// Create a new instance of this stream.
    pub const fn new(events: Receiver<FullTransactionEvent<T>>) -> Self {
        Self { events }
    }
}

impl<T: PoolTransaction> Stream for AllTransactionsEvents<T> {
    type Item = FullTransactionEvent<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().events.poll_recv(cx)
    }
}

/// A type that broadcasts [`TransactionEvent`] to installed listeners.
///
/// This is essentially a multi-producer, multi-consumer channel where each event is broadcast to
/// all active receivers.
#[derive(Debug)]
pub(crate) struct PoolEventBroadcast<T: PoolTransaction> {
    /// All listeners for all transaction events.
    all_events_broadcaster: AllPoolEventsBroadcaster<T>,
    /// All listeners for events for a certain transaction hash.
    broadcasters_by_hash: HashMap<TxHash, PoolEventBroadcaster>,
}

impl<T: PoolTransaction> Default for PoolEventBroadcast<T> {
    fn default() -> Self {
        Self {
            all_events_broadcaster: AllPoolEventsBroadcaster::default(),
            broadcasters_by_hash: HashMap::default(),
        }
    }
}

impl<T: PoolTransaction> PoolEventBroadcast<T> {
    /// Calls the broadcast callback with the `PoolEventBroadcaster` that belongs to the hash.
    fn broadcast_event(
        &mut self,
        hash: &TxHash,
        event: TransactionEvent,
        pool_event: FullTransactionEvent<T>,
    ) {
        // Broadcast to all listeners for the transaction hash.
        if let Entry::Occupied(mut sink) = self.broadcasters_by_hash.entry(*hash) {
            sink.get_mut().broadcast(event.clone());

            if sink.get().is_empty() || event.is_final() {
                sink.remove();
            }
        }

        // Broadcast to all listeners for all transactions.
        self.all_events_broadcaster.broadcast(pool_event);
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
    pub(crate) fn subscribe_all(&mut self) -> AllTransactionsEvents<T> {
        let (tx, rx) = tokio::sync::mpsc::channel(TX_POOL_EVENT_CHANNEL_SIZE);
        self.all_events_broadcaster.senders.push(tx);
        AllTransactionsEvents::new(rx)
    }

    /// Notify listeners about a transaction that was added to the pending queue.
    pub(crate) fn pending(&mut self, tx: &TxHash, replaced: Option<Arc<ValidPoolTransaction<T>>>) {
        self.broadcast_event(tx, TransactionEvent::Pending, FullTransactionEvent::Pending(*tx));

        if let Some(replaced) = replaced {
            // notify listeners that this transaction was replaced
            self.replaced(replaced, *tx);
        }
    }

    /// Notify listeners about a transaction that was replaced.
    pub(crate) fn replaced(&mut self, tx: Arc<ValidPoolTransaction<T>>, replaced_by: TxHash) {
        let transaction = Arc::clone(&tx);
        self.broadcast_event(
            tx.hash(),
            TransactionEvent::Replaced(replaced_by),
            FullTransactionEvent::Replaced { transaction, replaced_by },
        );
    }

    /// Notify listeners about a transaction that was added to the queued pool.
    pub(crate) fn queued(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Queued, FullTransactionEvent::Queued(*tx));
    }

    /// Notify listeners about a transaction that was propagated.
    pub(crate) fn propagated(&mut self, tx: &TxHash, peers: Vec<PropagateKind>) {
        let peers = Arc::new(peers);
        self.broadcast_event(
            tx,
            TransactionEvent::Propagated(Arc::clone(&peers)),
            FullTransactionEvent::Propagated(peers),
        );
    }

    /// Notify listeners about a transaction that was discarded.
    pub(crate) fn discarded(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Discarded, FullTransactionEvent::Discarded(*tx));
    }

    /// Notify listeners about a transaction that was invalid.
    pub(crate) fn invalid(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Invalid, FullTransactionEvent::Invalid(*tx));
    }

    /// Notify listeners that the transaction was mined
    pub(crate) fn mined(&mut self, tx: &TxHash, block_hash: B256) {
        self.broadcast_event(
            tx,
            TransactionEvent::Mined(block_hash),
            FullTransactionEvent::Mined { tx_hash: *tx, block_hash },
        );
    }
}

/// All Sender half(s) of the event channels for all transactions.
///
/// This mimics [`tokio::sync::broadcast`] but uses separate channels.
#[derive(Debug)]
struct AllPoolEventsBroadcaster<T: PoolTransaction> {
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<Sender<FullTransactionEvent<T>>>,
}

impl<T: PoolTransaction> Default for AllPoolEventsBroadcaster<T> {
    fn default() -> Self {
        Self { senders: Vec::new() }
    }
}

impl<T: PoolTransaction> AllPoolEventsBroadcaster<T> {
    // Broadcast an event to all listeners. Dropped listeners are silently evicted.
    fn broadcast(&mut self, event: FullTransactionEvent<T>) {
        self.senders.retain(|sender| match sender.try_send(event.clone()) {
            Ok(_) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Closed(_)) => false,
        })
    }
}

/// All Sender half(s) of the event channels for a specific transaction.
///
/// This mimics [`tokio::sync::broadcast`] but uses separate channels and is unbounded.
#[derive(Default, Debug)]
struct PoolEventBroadcaster {
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<UnboundedSender<TransactionEvent>>,
}

impl PoolEventBroadcaster {
    /// Returns `true` if there are no more listeners remaining.
    fn is_empty(&self) -> bool {
        self.senders.is_empty()
    }

    // Broadcast an event to all listeners. Dropped listeners are silently evicted.
    fn broadcast(&mut self, event: TransactionEvent) {
        self.senders.retain(|sender| sender.send(event.clone()).is_ok())
    }
}

/// An active listener for new pending transactions.
#[derive(Debug)]
pub(crate) struct PendingTransactionHashListener {
    pub(crate) sender: mpsc::Sender<TxHash>,
    /// Whether to include transactions that should not be propagated over the network.
    pub(crate) kind: TransactionListenerKind,
}

impl PendingTransactionHashListener {
    /// Attempts to send all hashes to the listener.
    ///
    /// Returns false if the channel is closed (receiver dropped)
    pub(crate) fn send_all(&self, hashes: impl IntoIterator<Item = TxHash>) -> bool {
        for tx_hash in hashes {
            match self.sender.try_send(tx_hash) {
                Ok(()) => {}
                Err(err) => {
                    return if matches!(err, mpsc::error::TrySendError::Full(_)) {
                        debug!(
                            target: "txpool",
                            "[{:?}] failed to send pending tx; channel full",
                            tx_hash,
                        );
                        true
                    } else {
                        false
                    }
                }
            }
        }
        true
    }
}

/// An active listener for new pending transactions.
#[derive(Debug)]
pub(crate) struct TransactionListener<T: PoolTransaction> {
    pub(crate) sender: mpsc::Sender<NewTransactionEvent<T>>,
    /// Whether to include transactions that should not be propagated over the network.
    pub(crate) kind: TransactionListenerKind,
}

impl<T: PoolTransaction> TransactionListener<T> {
    /// Attempts to send the event to the listener.
    ///
    /// Returns false if the channel is closed (receiver dropped)
    pub(crate) fn send(&self, event: NewTransactionEvent<T>) -> bool {
        self.send_all(std::iter::once(event))
    }

    /// Attempts to send all events to the listener.
    ///
    /// Returns false if the channel is closed (receiver dropped)
    pub(crate) fn send_all(
        &self,
        events: impl IntoIterator<Item = NewTransactionEvent<T>>,
    ) -> bool {
        for event in events {
            match self.sender.try_send(event) {
                Ok(()) => {}
                Err(err) => {
                    return if let mpsc::error::TrySendError::Full(event) = err {
                        debug!(
                            target: "txpool",
                            "[{:?}] failed to send pending tx; channel full",
                            event.transaction.hash(),
                        );
                        true
                    } else {
                        false
                    }
                }
            }
        }
        true
    }
}

/// An active listener for new blobs
#[derive(Debug)]
pub(crate) struct BlobTransactionSidecarListener {
    pub(crate) sender: mpsc::Sender<NewBlobSidecar>,
}

/// Determines what kind of new transactions should be emitted by a stream of transactions.
///
/// This gives control whether to include transactions that are allowed to be propagated.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TransactionListenerKind {
    /// Any new pending transactions
    All,
    /// Only transactions that are allowed to be propagated.
    ///
    /// See also [`ValidPoolTransaction`]
    PropagateOnly,
}

impl TransactionListenerKind {
    /// Returns true if we're only interested in transactions that are allowed to be propagated.
    #[inline]
    pub const fn is_propagate_only(&self) -> bool {
        matches!(self, Self::PropagateOnly)
    }
}
