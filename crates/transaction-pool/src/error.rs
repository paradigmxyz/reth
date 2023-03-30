//! Transaction pool errors

use reth_primitives::{Address, InvalidTransactionError, TxHash};

/// Transaction pool result type.
pub type PoolResult<T> = Result<T, PoolError>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// Same transaction already imported
    #[error("[{0:?}] Already imported")]
    AlreadyImported(TxHash),
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("[{0:?}]: insufficient gas price to replace existing transaction.")]
    ReplacementUnderpriced(TxHash),
    /// The fee cap of the transaction is below the minimum fee cap determined by the protocol
    #[error("[{0:?}] Transaction feeCap {1} below chain minimum.")]
    FeeCapBelowMinimumProtocolFeeCap(TxHash, u128),
    /// Thrown when the number of unique transactions of a sender exceeded the slot capacity.
    #[error("{0:?} identified as spammer. Transaction {1:?} rejected.")]
    SpammerExceededCapacity(Address, TxHash),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("[{0:?}] Transaction discarded outright due to pool size constraints.")]
    DiscardedOnInsert(TxHash),
    /// Thrown when the transaction is considered invalid.
    #[error("[{0:?}] {1:?}")]
    InvalidTransaction(TxHash, InvalidPoolTransactionError),
    /// Any other error that occurred while inserting/validating a transaction. e.g. IO database
    /// error
    #[error("[{0:?}] {1:?}")]
    Other(TxHash, Box<dyn std::error::Error + Send + Sync>),
}

// === impl PoolError ===

impl PoolError {
    /// Returns the hash of the transaction that resulted in this error.
    pub fn hash(&self) -> &TxHash {
        match self {
            PoolError::AlreadyImported(hash) => hash,
            PoolError::ReplacementUnderpriced(hash) => hash,
            PoolError::FeeCapBelowMinimumProtocolFeeCap(hash, _) => hash,
            PoolError::SpammerExceededCapacity(_, hash) => hash,
            PoolError::DiscardedOnInsert(hash) => hash,
            PoolError::InvalidTransaction(hash, _) => hash,
            PoolError::Other(hash, _) => hash,
        }
    }

    /// Returns `true` if the error was caused by a transaction that is considered bad in the
    /// context of the transaction pool.
    ///
    /// Not all error variants are caused by the incorrect composition of the transaction (See also
    /// [InvalidPoolTransactionError]) and can be caused by the current state of the transaction
    /// pool. For example the transaction pool is already full or the error was caused my an
    /// internal error, such as database errors.
    ///
    /// This function returns true only if the transaction will never make it into the pool because
    /// its composition is invalid and the original sender should have detected this as well. This
    /// is used to determine whether the original sender should be penalized for sending an
    /// erroneous transaction.
    #[inline]
    pub fn is_bad_transaction(&self) -> bool {
        match self {
            PoolError::AlreadyImported(_) => {
                // already imported but not bad
                false
            }
            PoolError::ReplacementUnderpriced(_) => {
                // already imported but not bad
                false
            }
            PoolError::FeeCapBelowMinimumProtocolFeeCap(_, _) => {
                // fee cap of the tx below the technical minimum determined by the protocol: always
                // invalid
                true
            }
            PoolError::SpammerExceededCapacity(_, _) => {
                // spammer detected
                true
            }
            PoolError::DiscardedOnInsert(_) => {
                // valid tx but dropped due to size constraints
                false
            }
            PoolError::InvalidTransaction(_, _) => {
                // transaction rejected because it violates constraints
                true
            }
            PoolError::Other(_, _) => {
                // internal error unrelated to the transaction
                false
            }
        }
    }
}

/// Represents errors that can happen when validating transactions for the pool
///
/// See [TransactionValidator](crate::TransactionValidator).
#[derive(Debug, Clone, thiserror::Error)]
pub enum InvalidPoolTransactionError {
    /// Hard consensus errors
    #[error(transparent)]
    Consensus(#[from] InvalidTransactionError),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the size limits of the pool.
    #[error("Transaction's gas limit {0} exceeds block's gas limit {1}.")]
    ExceedsGasLimit(u64, u64),
    /// Thrown when a new transaction is added to the pool, but then immediately discarded to
    /// respect the max_init_code_size.
    #[error("Transaction's size {0} exceeds max_init_code_size {1}.")]
    ExceedsMaxInitCodeSize(usize, usize),
    /// Thrown if the transaction contains an invalid signature
    #[error("Invalid sender")]
    AccountNotFound,
    /// Thrown if the input data of a transaction is greater
    /// than some meaningful limit a user might use. This is not a consensus error
    /// making the transaction invalid, rather a DOS protection.
    #[error("Input data too large")]
    OversizedData(usize, usize),
}
