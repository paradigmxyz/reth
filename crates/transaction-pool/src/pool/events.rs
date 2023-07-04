use crate::traits::PropagateKind;
use reth_primitives::{TxHash, H256};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Wrapper around a transaction hash and the event that happened to it.
#[derive(Debug)]
pub struct PoolTransactionEvent(TxHash, TransactionEvent);

impl PoolTransactionEvent {
    /// Create a new transaction event.
    pub fn new(hash: TxHash, event: TransactionEvent) -> Self {
        Self(hash, event)
    }

    /// The hash of the transaction this event is about.
    pub fn hash(&self) -> TxHash {
        self.0
    }

    /// The event that happened to the transaction.
    pub fn event(&self) -> &TransactionEvent {
        &self.1
    }

    /// Split the event into its components.
    pub fn split(self) -> (TxHash, TransactionEvent) {
        (self.0, self.1)
    }
}

/// Various events that describe status changes of a transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TransactionEvent {
    /// Transaction has been added to the pending pool.
    Pending,
    /// Transaction has been added to the queued pool.
    Queued,
    /// Transaction has been included in the block belonging to this hash.
    Mined(H256),
    /// Transaction has been replaced by the transaction belonging to the hash.
    ///
    /// E.g. same (sender + nonce) pair
    Replaced(TxHash),
    /// Transaction was dropped due to configured limits.
    Discarded,
    /// Transaction became invalid indefinitely.
    Invalid,
    /// Transaction was propagated to peers.
    Propagated(Arc<Vec<PropagateKind>>),
}

impl TransactionEvent {
    /// Returns `true` if the event is final and no more events are expected for this transaction
    /// hash.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            TransactionEvent::Replaced(_) |
                TransactionEvent::Mined(_) |
                TransactionEvent::Discarded
        )
    }
}
