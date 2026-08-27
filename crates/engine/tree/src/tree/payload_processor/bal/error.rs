//! Errors for the BAL execution path.

use reth_errors::{BlockExecutionError, ConsensusError, ProviderError};
use revm::bytecode::BytecodeDecodeError;

/// Errors surfaced by `execute_block`.
#[derive(Debug, thiserror::Error)]
pub enum BalExecutionError {
    /// Block violated consensus rules while running the BAL path.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// The block access list contains bytecode that cannot result from valid execution.
    #[error("invalid block access list: {0}")]
    BlockAccessListInvalid(#[from] BytecodeDecodeError),
    /// Worker or canonical EVM failure.
    #[error("evm execution failed: {0}")]
    Execution(#[from] BlockExecutionError),
    /// Provider setup failed before EVM execution could start.
    #[error("provider setup failed: {0}")]
    Provider(#[from] ProviderError),
    /// BAL execution failed before it reached EVM execution.
    #[error(transparent)]
    Other(#[from] Box<dyn core::error::Error + Send + Sync + 'static>),
}

impl BalExecutionError {
    /// Create an [`Self::Other`] error from any boxed-error-compatible value.
    pub(crate) fn other<E>(error: E) -> Self
    where
        E: Into<Box<dyn core::error::Error + Send + Sync + 'static>>,
    {
        Self::Other(error.into())
    }
}
