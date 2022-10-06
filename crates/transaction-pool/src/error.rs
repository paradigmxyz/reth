//! Transaction pool errors

use reth_primitives::BlockID;

/// Transaction pool result type.
pub type PoolResult<T> = Result<T, PoolError>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("Tx: insufficient gas price to replace existing transaction")]
    ReplacementUnderpriced,
    /// Encountered a transaction that was already added into the poll
    #[error("[{0:?}] Already added")]
    AlreadyAdded(Box<dyn std::any::Any + Send + Sync>),
    /// Encountered a cycle in the graph pool
    #[error("Transaction with cyclic dependent transactions")]
    CyclicTransaction,
    /// Thrown if no number was found for the given block id
    #[error("Invalid block id: {0:?}")]
    BlockNumberNotFound(BlockID),
}
