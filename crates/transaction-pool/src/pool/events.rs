use crate::traits::PropagateKind;
use reth_primitives::{TxHash, H256};
use serde::{Deserialize, Serialize};

/// Various events that describe status changes of a transaction.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
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
    Propagated(Vec<PropagateKind>),
}
