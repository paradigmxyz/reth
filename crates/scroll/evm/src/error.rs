use derive_more::{Display, From};
use reth_consensus::ConsensusError;
use reth_evm::execute::BlockExecutionError;

/// Execution error for Scroll.
#[derive(thiserror::Error, Display, From, Debug)]
pub enum ScrollBlockExecutionError {
    /// Error occurred at fork transition.
    #[display("failed to apply fork: {_0}")]
    Fork(ForkError),
    /// Error occurred at consensus verification.
    #[display("failed to validate block: {_0}")]
    Consensus(ConsensusError),
}

impl ScrollBlockExecutionError {
    /// Build a [`ScrollBlockExecutionError::Fork`] from the provider fork error.
    pub fn fork(error: ForkError) -> Self {
        Self::Fork(error)
    }

    /// Build a [`ScrollBlockExecutionError::Consensus`] from the provider consensus error.
    pub fn consensus(error: ConsensusError) -> Self {
        Self::Consensus(error)
    }
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
