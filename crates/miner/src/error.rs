//! Error types emitted by types or implementations of this crate.

use revm_primitives::EVMError;
use reth_primitives::H256;
use tokio::sync::oneshot;

/// Possible error variants during payload building.
#[derive(Debug, thiserror::Error)]
pub enum PayloadBuilderError {
    /// Thrown whe the parent block is missing.
    #[error("missing parent block {0:?}")]
    MissingParentBlock(H256),
    /// An oneshot channels has been closed.
    #[error("sender has been dropped")]
    ChannelClosed,
    /// Other internal error
    #[error(transparent)]
    Internal(#[from] reth_interfaces::Error),

    // TODO move to standalone error type specific to job
    /// Thrown if a running build job has been cancelled.
    #[error("build job cancelled during execution")]
    BuildJobCancelled,
    /// Unrecoverable error during evm execution.
    #[error(transparent)]
    EvmExecutionError(#[from] EVMError<reth_interfaces::Error>)
}

impl From<oneshot::error::RecvError> for PayloadBuilderError {
    fn from(_: oneshot::error::RecvError) -> Self {
        PayloadBuilderError::ChannelClosed
    }
}
