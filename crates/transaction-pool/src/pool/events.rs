use serde::{Deserialize, Serialize};

/// Various events that describe status changes of a transaction.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum TransactionEvent<Hash, BlockHash> {
    /// Transaction has been added to the pending pool.
    Pending,
    /// Transaction has been added to the queued pool.
    Queued,
    /// Transaction has been included in the block belonging to this hash.
    Included(BlockHash),
    /// Transaction has been replaced by the transaction belonging to the hash.
    ///
    /// E.g. same (sender + nonce) pair
    Replaced(Hash),
    /// Transaction was dropped due to configured limits.
    Dropped,
    /// Transaction became invalid indefinitely.
    Invalid,
    // TODO Timedout?, broadcasted(peers)
}
