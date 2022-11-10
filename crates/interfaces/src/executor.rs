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
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Example of error
    #[error("Example of error.")]
    VerificationFailed,
}
