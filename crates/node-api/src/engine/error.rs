//! Defines a payload validation error type
use thiserror::Error;

/// Thrown when the payload or attributes are known to be invalid before processing.
///
/// This is used mainly for
/// [validate_version_specific_fields](crate::validate_version_specific_fields), which validates
/// both execution payloads and forkchoice update attributes with respect to a method version.
#[derive(Error, Debug)]
pub enum EngineObjectValidationError {
    /// Thrown when the underlying validation error occured while validating an `ExecutionPayload`.
    #[error("Payload validation error: {0}")]
    Payload(VersionSpecificValidationError),

    /// Thrown when the underlying validation error occured while validating a `PayloadAttributes`.
    #[error("Payload attributes validation error: {0}")]
    PayloadAttributes(VersionSpecificValidationError),

    /// Thrown if `PayloadAttributes` or `ExecutionPayload` were provided with a timestamp, but the
    /// version of the engine method called is meant for a fork that occurs after the provided
    /// timestamp.
    #[error("Unsupported fork")]
    UnsupportedFork,
    /// Another type of error that is not covered by the above variants.
    #[error("Invalid params: {0}")]
    InvalidParams(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Thrown when validating an execution payload OR payload attributes fails due to:
/// * The existence of a new field that is not supported in the given engine method version, or
/// * The absence of a field that is required in the given engine method version
#[derive(Error, Debug)]
pub enum VersionSpecificValidationError {
    /// Thrown if the pre-V3 `PayloadAttributes` or `ExecutionPayload` contains a parent beacon
    /// block root
    #[error("parent beacon block root not supported before V3")]
    ParentBeaconBlockRootNotSupportedBeforeV3,
    /// Thrown if engine_forkchoiceUpdatedV1 or engine_newPayloadV1 contains withdrawals
    #[error("withdrawals not supported in V1")]
    WithdrawalsNotSupportedInV1,
    /// Thrown if engine_forkchoiceUpdated or engine_newPayload contains no withdrawals after
    /// Shanghai
    #[error("no withdrawals post-Shanghai")]
    NoWithdrawalsPostShanghai,
    /// Thrown if engine_forkchoiceUpdated or engine_newPayload contains withdrawals before
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
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::InvalidParams(Box::new(error))
    }
}
