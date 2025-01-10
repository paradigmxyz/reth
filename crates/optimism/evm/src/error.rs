//! Error types for the Optimism EVM module.

use alloc::string::String;
use reth_evm::execute::BlockExecutionError;

/// Optimism Block Executor Errors
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpBlockExecutionError {
    /// Error when trying to parse L1 block info
    #[error("could not get L1 block info from L2 block: {message}")]
    L1BlockInfoError {
        /// The inner error message
        message: String,
    },
    /// Thrown when force deploy of create2deployer code fails.
    #[error("failed to force create2deployer account code")]
    ForceCreate2DeployerFail,
    /// Thrown when a blob transaction is included in a sequencer's block.
    #[error("blob transaction included in sequencer block")]
    BlobTransactionRejected,
    /// Thrown when a database account could not be loaded.
    #[error("failed to load account {_0}")]
    AccountLoadFailed(alloy_primitives::Address),
}

impl From<OpBlockExecutionError> for BlockExecutionError {
    fn from(err: OpBlockExecutionError) -> Self {
        Self::other(err)
    }
}
