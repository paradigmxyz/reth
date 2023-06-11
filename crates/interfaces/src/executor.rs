use reth_primitives::{BlockHash, BlockNumHash, Bloom, H256};
use thiserror::Error;

/// Transaction validation errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    #[error("EVM reported invalid transaction ({hash:?}): {message}")]
    EVM { hash: H256, message: String },
    #[error("Verification failed")]
    VerificationFailed,
    #[error("Fatal internal error")]
    ExecutionFatalError,
    #[error("Failed to recover sender for transaction")]
    SenderRecoveryError,
    #[error("Receipt root {got:?} is different than expected {expected:?}.")]
    ReceiptRootDiff { got: H256, expected: H256 },
    #[error("Header bloom filter {got:?} is different than expected {expected:?}.")]
    BloomLogDiff { got: Box<Bloom>, expected: Box<Bloom> },
    #[error("Transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThanAvailableBlockGas {
        transaction_gas_limit: u64,
        block_available_gas: u64,
    },
    #[error("Block gas used {got} is different from expected gas used {expected}.")]
    BlockGasUsed { got: u64, expected: u64 },
    #[error("Block {hash:?} is pre merge")]
    BlockPreMerge { hash: H256 },
    #[error("Missing total difficulty")]
    MissingTotalDifficulty { hash: H256 },
}

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BlockExecutionError {
    #[error(transparent)]
    Validation(#[from] BlockValidationError),

    // === misc provider error ===
    #[error("Provider error")]
    ProviderError,

    // === transaction errors ===
    #[error("Transaction error on revert: {inner:?}")]
    CanonicalRevert { inner: String },
    #[error("Transaction error on commit: {inner:?}")]
    CanonicalCommit { inner: String },
    #[error("Transaction error on pipeline status update: {inner:?}")]
    PipelineStatusUpdate { inner: String },
    #[error("DB Error during transaction execution: {inner:?}")]
    DBError { inner: String },
}
