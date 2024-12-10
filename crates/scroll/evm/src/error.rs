use derive_more::{Display, From};
use reth_evm::execute::BlockExecutionError;

/// Execution error for Scroll.
#[derive(thiserror::Error, Display, From, Debug)]
pub enum ScrollBlockExecutionError {
    /// Error occurred at fork transition.
    #[display("failed to apply fork: {_0}")]
    Fork(ForkError),
}

impl From<ScrollBlockExecutionError> for BlockExecutionError {
    fn from(value: ScrollBlockExecutionError) -> Self {
        Self::other(value)
    }
}

/// Scroll fork error.
#[derive(Debug, Display)]
pub enum ForkError {
    /// Error occurred at Curie fork.
    Curie(String),
}

impl From<ForkError> for BlockExecutionError {
    fn from(value: ForkError) -> Self {
        ScrollBlockExecutionError::Fork(value).into()
    }
}
