//! Transaction pool errors

use reth_primitives::{Address, TxHash, U256};

/// Transaction pool result type.
pub type PoolResult<T> = Result<T, PoolError>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("[{0:?}]: insufficient gas price to replace existing transaction.")]
    ReplacementUnderpriced(TxHash),
    /// Encountered a transaction that was already added into the poll
    #[error("[{0:?}] Transaction feeCap {1} below chain minimum.")]
    ProtocolFeeCapTooLow(TxHash, U256),
    /// Thrown when the number of unique transactions of a sender exceeded the slot capacity.
    #[error("{0:?} identified as spammer. Transaction {1:?} rejected.")]
    SpammerExceededCapacity(Address, TxHash),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("[{0:?}] Transaction discarded outright due to pool size constraints.")]
    DiscardedOnInsert(TxHash),
}

// === impl PoolError ===

impl PoolError {
    /// Returns the hash of the transaction that resulted in this error.
    pub fn hash(&self) -> &TxHash {
        match self {
            PoolError::ReplacementUnderpriced(hash) => hash,
            PoolError::ProtocolFeeCapTooLow(hash, _) => hash,
            PoolError::SpammerExceededCapacity(_, hash) => hash,
            PoolError::DiscardedOnInsert(hash) => hash,
        }
    }
}
