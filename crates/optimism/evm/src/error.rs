//! Error types for the Optimism EVM module.

use alloc::string::String;
use reth_evm::execute::BlockExecutionError;

/// Optimism Block Executor Errors
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Display)]
pub enum OpBlockExecutionError {
    /// Error when trying to parse L1 block info
    #[display("could not get L1 block info from L2 block: {message}")]
    L1BlockInfoError {
        /// The inner error message
        message: String,
    },
    /// Thrown when force deploy of create2deployer code fails.
    #[display("failed to force create2deployer account code")]
    ForceCreate2DeployerFail,
    /// Thrown when a blob transaction is included in a sequencer's block.
    #[display("blob transaction included in sequencer block")]
    BlobTransactionRejected,
    /// Thrown when a database account could not be loaded.
    #[display("failed to load account {_0}")]
    AccountLoadFailed(alloy_primitives::Address),
}

impl core::error::Error for OpBlockExecutionError {}

impl From<OpBlockExecutionError> for BlockExecutionError {
    fn from(err: OpBlockExecutionError) -> Self {
        Self::other(err)
    }
}
