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
    /// Build cancelled
    #[error("build outcome cancelled")]
    BuildOutcomeCancelled,
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
    Other(Box<dyn core::error::Error + Send + Sync>),
}

impl PayloadBuilderError {
    /// Create a new error from a boxed error.
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
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

/// Thrown when the payload or attributes are known to be invalid before processing.
///
/// This is used mainly for
/// [`validate_version_specific_fields`](crate::validate_version_specific_fields), which validates
/// both execution payloads and forkchoice update attributes with respect to a method version.
#[derive(thiserror::Error, Debug)]
pub enum EngineObjectValidationError {
    /// Thrown when the underlying validation error occurred while validating an
    /// `ExecutionPayload`.
    #[error("Payload validation error: {0}")]
    Payload(VersionSpecificValidationError),

    /// Thrown when the underlying validation error occurred while validating a
    /// `PayloadAttributes`.
    #[error("Payload attributes validation error: {0}")]
    PayloadAttributes(VersionSpecificValidationError),

    /// Thrown if `PayloadAttributes` or `ExecutionPayload` were provided with a timestamp, but the
    /// version of the engine method called is meant for a fork that occurs after the provided
    /// timestamp.
    #[error("Unsupported fork")]
    UnsupportedFork,
    /// Another type of error that is not covered by the above variants.
    #[error("Invalid params: {0}")]
    InvalidParams(#[from] Box<dyn core::error::Error + Send + Sync>),
}

/// Thrown when validating an execution payload OR payload attributes fails due to:
/// * The existence of a new field that is not supported in the given engine method version, or
/// * The absence of a field that is required in the given engine method version
#[derive(thiserror::Error, Debug)]
pub enum VersionSpecificValidationError {
    /// Thrown if the pre-V3 `PayloadAttributes` or `ExecutionPayload` contains a parent beacon
    /// block root
    #[error("parent beacon block root not supported before V3")]
    ParentBeaconBlockRootNotSupportedBeforeV3,
    /// Thrown if `engine_forkchoiceUpdatedV1` or `engine_newPayloadV1` contains withdrawals
    #[error("withdrawals not supported in V1")]
    WithdrawalsNotSupportedInV1,
    /// Thrown if `engine_forkchoiceUpdated` or `engine_newPayload` contains no withdrawals after
    /// Shanghai
    #[error("no withdrawals post-Shanghai")]
    NoWithdrawalsPostShanghai,
    /// Thrown if `engine_forkchoiceUpdated` or `engine_newPayload` contains withdrawals before
    /// Shanghai
    #[error("withdrawals pre-Shanghai")]
    HasWithdrawalsPreShanghai,
    /// Thrown if the `PayloadAttributes` or `ExecutionPayload` contains no parent beacon block
    /// root after Cancun
    #[error("no parent beacon block root post-cancun")]
    NoParentBeaconBlockRootPostCancun,
}

impl EngineObjectValidationError {
    /// Creates an instance of the `InvalidParams` variant with the given error.
    pub fn invalid_params<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::InvalidParams(Box::new(error))
    }
}
