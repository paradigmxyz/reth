use crate::{traits::PropagateKind, PoolTransaction, ValidPoolTransaction};
use reth_primitives::{TxHash, B256};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// An event that happened to a transaction and contains its full body where possible.
#[derive(Debug)]
pub enum FullTransactionEvent<T: PoolTransaction> {
    /// Transaction has been added to the pending pool.
    Pending(TxHash),
    /// Transaction has been added to the queued pool.
    Queued(TxHash),
    /// Transaction has been included in the block belonging to this hash.
    Mined {
        /// The hash of the mined transaction.
        tx_hash: TxHash,
        /// The hash of the mined block that contains the transaction.
        block_hash: B256,
    },
    /// Transaction has been replaced by the transaction belonging to the hash.
    ///
    /// E.g. same (sender + nonce) pair
    Replaced {
        /// The transaction that was replaced.
        transaction: Arc<ValidPoolTransaction<T>>,
        /// The transaction that replaced the event subject.
        replaced_by: TxHash,
    },
    /// Transaction was dropped due to configured limits.
    Discarded(TxHash),
    /// Transaction became invalid indefinitely.
    Invalid(TxHash),
    /// Transaction was propagated to peers.
    Propagated(Arc<Vec<PropagateKind>>),
}

impl<T: PoolTransaction> Clone for FullTransactionEvent<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Pending(hash) => Self::Pending(*hash),
            Self::Queued(hash) => Self::Queued(*hash),
            Self::Mined { tx_hash, block_hash } => {
                Self::Mined { tx_hash: *tx_hash, block_hash: *block_hash }
            }
            Self::Replaced { transaction, replaced_by } => {
                Self::Replaced { transaction: Arc::clone(transaction), replaced_by: *replaced_by }
            }
            Self::Discarded(hash) => Self::Discarded(*hash),
            Self::Invalid(hash) => Self::Invalid(*hash),
            Self::Propagated(propagated) => Self::Propagated(Arc::clone(propagated)),
        }
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
    Mined(B256),
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
    pub const fn is_final(&self) -> bool {
        matches!(
            self,
            TransactionEvent::Replaced(_) |
                TransactionEvent::Mined(_) |
                TransactionEvent::Discarded
        )
    }
}
