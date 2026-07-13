//! Errors for the BAL execution path.

use reth_errors::{BlockExecutionError, ConsensusError, ProviderError};

/// Errors surfaced by parallel BAL block execution.
#[derive(Debug, thiserror::Error)]
pub enum BalExecutionError {
    /// Block violated consensus rules while running the BAL path.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
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
    pub(crate) fn other<E>(error: E) -> Self
    where
        E: Into<Box<dyn core::error::Error + Send + Sync + 'static>>,
    {
        Self::Other(error.into())
    }
}
