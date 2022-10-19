//! Transaction pool errors

use reth_primitives::{Address, BlockID, TxHash, U256};

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
}
