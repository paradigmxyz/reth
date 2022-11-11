use async_trait::async_trait;
use reth_primitives::Block;
use thiserror::Error;

/// Takes block and executes it, returns error
#[async_trait]
pub trait BlockExecutor {
    /// Execute block
    async fn execute(&self, _block: Block) -> Error {
        Error::VerificationFailed
    }
}

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("Example of error.")]
    VerificationFailed,
    #[error("Fatal internal error")]
    ExecutionFatalError,
    #[error("Receipt cumulative gas used {got:?} is different from expected: {expected:?}")]
    ReceiptCumulativeGasUsedDiff { got: u64, expected: u64 },
    #[error("Receipt log count {got:?} is different from expected {expected:?}.")]
    ReceiptLogCountDiff { got: usize, expected: usize },
    #[error("Receipt log is different.")]
    ReceiptLogDiff,
    #[error("Receipt log is different.")]
    ExecutionSuccessDiff { got: bool, expected: bool },
}
