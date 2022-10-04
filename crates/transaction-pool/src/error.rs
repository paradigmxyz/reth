//! Transaction pool errors

use reth_primitives::BlockId;

/// Transaction pool result type.
pub type PoolResult<T> = Result<T, PoolError>;

/// All errors the Transaction pool can throw.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// Thrown if a replacement transaction's gas price is below the already imported transaction
    #[error("Tx: insufficient gas price to replace existing transaction")]
    // #[error("Tx: [{0:?}] insufficient gas price to replace existing transaction")]
    // ReplacementUnderpriced(Box<PoolTransaction>),
    ReplacementUnderpriced,
    // TODO make error generic over `Transaction`
    #[error("[{0:?}] Already added")]
    AlreadyAdded(Box<dyn std::any::Any + Send + Sync>),
    #[error("Transaction with cyclic dependent transactions")]
    CyclicTransaction,
    /// Thrown if no number was found for the given block id
    #[error("Invalid block id: {0:?}")]
    BlockNumberNotFound(BlockId),
}
