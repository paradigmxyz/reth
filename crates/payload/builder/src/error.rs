//! Error types emitted by types or implementations of this crate.

use alloy_primitives::B256;
use reth_errors::{ProviderError, RethError};
use reth_primitives::revm_primitives::EVMError;
use reth_transaction_pool::BlobStoreError;
use tokio::sync::oneshot;

/// Possible error variants during payload building.
#[derive(Debug, thiserror::Error)]
pub enum PayloadBuilderError {
    /// Thrown when the parent block is missing.
    #[error("missing parent block {0}")]
    MissingParentBlock(B256),
    /// An oneshot channels has been closed.
    #[error("sender has been dropped")]
    ChannelClosed,
    /// If there's no payload to resolve.
    #[error("missing payload")]
    MissingPayload,
    /// Error occurring in the blob store.
    #[error(transparent)]
    BlobStore(#[from] BlobStoreError),
    /// Other internal error
    #[error(transparent)]
    Internal(#[from] RethError),
    /// Unrecoverable error during evm execution.
    #[error("evm execution error: {0}")]
    EvmExecutionError(EVMError<ProviderError>),
    /// Thrown if the payload requests withdrawals before Shanghai activation.
    #[error("withdrawals set before Shanghai activation")]
    WithdrawalsBeforeShanghai,
    /// Any other payload building errors.
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl PayloadBuilderError {
    /// Create a new error from a boxed error.
    pub fn other<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }
}

impl From<ProviderError> for PayloadBuilderError {
    fn from(error: ProviderError) -> Self {
        Self::Internal(RethError::Provider(error))
    }
}

impl From<oneshot::error::RecvError> for PayloadBuilderError {
    fn from(_: oneshot::error::RecvError) -> Self {
        Self::ChannelClosed
    }
}
