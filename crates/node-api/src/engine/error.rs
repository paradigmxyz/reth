//! Defines a payload validation error type
use thiserror::Error;

/// Thrown when the payload or attributes are known to be invalid before processing.
#[derive(Error, Debug)]
pub enum AttributesValidationError {
    /// Thrown if `PayloadAttributes` provided in engine_forkchoiceUpdated before V3 contains a
    /// parent beacon block root
    #[error("parent beacon block root not supported before V3")]
    ParentBeaconBlockRootNotSupportedBeforeV3,
    /// Thrown if engine_forkchoiceUpdatedV1 contains withdrawals
    #[error("withdrawals not supported in V1")]
    WithdrawalsNotSupportedInV1,
    /// Thrown if engine_forkchoiceUpdated contains no withdrawals after Shanghai
    #[error("no withdrawals post-Shanghai")]
    NoWithdrawalsPostShanghai,
    /// Thrown if engine_forkchoiceUpdated contains withdrawals before Shanghai
    #[error("withdrawals pre-Shanghai")]
    HasWithdrawalsPreShanghai,
    /// Thrown if the `PayloadAttributes` provided in engine_forkchoiceUpdated contains no parent
    /// beacon block root after Cancun
    #[error("no parent beacon block root post-cancun")]
    NoParentBeaconBlockRootPostCancun,
    /// Thrown if `PayloadAttributes` were provided with a timestamp, but the version of the engine
    /// method called is meant for a fork that occurs after the provided timestamp.
    #[error("Unsupported fork")]
    UnsupportedFork,
    /// Another type of error that is not covered by the above variants.
    #[error("Invalid params: {0}")]
    InvalidParams(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl AttributesValidationError {
    /// Creates an instance of the `InvalidParams` variant with the given error.
    pub fn invalid_params<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::InvalidParams(Box::new(error))
    }
}
