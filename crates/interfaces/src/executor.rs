use async_trait::async_trait;
use reth_primitives::Block;

/// Takes block and executes it, returns error
#[async_trait]
pub trait BlockExecutor {
    /// Execute block
    async fn execute(&self, _block: Block) -> Error {
        Error::VerificationFailed
    }
}

/// BlockExecutor Errors
#[derive(Debug, Clone)]
pub enum Error {
    /// Example of error
    VerificationFailed,
}
