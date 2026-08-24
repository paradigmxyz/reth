use alloc::boxed::Box;
use alloy_rpc_types_engine::ForkchoiceUpdateError;
use reth_errors::{BlockExecutionError, BlockValidationError, ConsensusError, ProviderError};
use reth_execution_errors::InternalBlockExecutionError;

/// Represents all error cases when handling a new payload.
///
/// This represents all possible error cases that must be returned as JSON RPC errors back to the
/// beacon node.
#[derive(Debug, thiserror::Error)]
pub enum BeaconOnNewPayloadError {
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
    /// Thrown when the payload params are malformed, e.g. a field's raw bytes cannot be decoded.
    ///
    /// Per the engine API spec this must be rejected with an invalid params error instead of an
    /// `INVALID` payload status.
    #[error(transparent)]
    InvalidParams(Box<dyn core::error::Error + Send + Sync>),
    /// An internal error occurred, not necessarily related to the payload.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl BeaconOnNewPayloadError {
    /// Create a new internal error.
    pub fn internal<E: core::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Internal(Box::new(e))
    }
}

impl From<InsertBlockFatalError> for BeaconOnNewPayloadError {
    fn from(error: InsertBlockFatalError) -> Self {
        match error {
            InsertBlockFatalError::InvalidParams(err) => Self::InvalidParams(err),
            error => Self::internal(error),
        }
    }
}

/// Represents error cases for an applied forkchoice update.
///
/// This represents all possible error cases, that must be returned as JSON RPC errors back to the
/// beacon node.
#[derive(Debug, thiserror::Error)]
pub enum BeaconForkChoiceUpdateError {
    /// Thrown when a forkchoice update resulted in an error.
    #[error("forkchoice update error: {0}")]
    ForkchoiceUpdateError(#[from] ForkchoiceUpdateError),
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
    /// An internal error occurred, not necessarily related to the update.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl BeaconForkChoiceUpdateError {
    /// Create a new internal error.
    pub fn internal<E: core::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Internal(Box::new(e))
    }
}

/// All error variants possible when inserting or validating a block.
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockErrorKind {
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Block execution failed.
    #[error(transparent)]
    Execution(#[from] BlockExecutionError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Other errors.
    #[error(transparent)]
    Other(#[from] Box<dyn core::error::Error + Send + Sync + 'static>),
}

impl InsertBlockErrorKind {
    /// Returns whether the error was caused by an invalid block.
    pub const fn is_validation_error(&self) -> bool {
        matches!(self, Self::Consensus(_) | Self::Execution(BlockExecutionError::Validation(_)))
    }

    /// Returns an [`InsertBlockValidationError`] if the error is caused by an invalid block.
    ///
    /// Returns an [`InsertBlockFatalError`] if the failure is not attributable to the block
    /// itself, either an internal error or malformed request params.
    ///
    /// This split decides how `newPayload` responds: validation errors become an `INVALID`
    /// payload status and mark the block hash as invalid, while fatal errors are returned as
    /// actual errors to the caller. This distinction is required because responding `INVALID`
    /// has consensus meaning (the block is rejected and its hash cached as invalid), which must
    /// not happen for failures the block is not responsible for.
    pub fn ensure_validation_error(
        self,
    ) -> Result<InsertBlockValidationError, InsertBlockFatalError> {
        match self {
            // Undecodable block access list bytes are malformed request params, not an invalid
            // block, and must be rejected with an invalid params error instead of an `INVALID`
            // payload status.
            Self::Consensus(ConsensusError::BlockAccessListDecode(err)) => {
                Err(InsertBlockFatalError::InvalidParams(Box::new(err)))
            }
            Self::Consensus(err) => Ok(InsertBlockValidationError::Consensus(err)),
            Self::Execution(err) => match err {
                BlockExecutionError::Validation(err) => {
                    Ok(InsertBlockValidationError::Validation(err))
                }
                BlockExecutionError::Internal(error) => {
                    Err(InsertBlockFatalError::BlockExecutionError(error))
                }
            },
            Self::Provider(err) => Err(InsertBlockFatalError::Provider(err)),
            Self::Other(err) => Err(InternalBlockExecutionError::Other(err).into()),
        }
    }
}

/// Error variants that are not caused by invalid blocks.
///
/// "Fatal" means block processing failed for a reason other than the block itself being invalid.
/// This includes errors caused by additional payload data that is not part of the block, such as
/// undecodable block access list bytes: their malformation says nothing about the validity of
/// the block and is therefore treated differently with respect to the `PayloadStatus`.
///
/// These failures must not be answered with an `INVALID` payload status or mark the block hash
/// as invalid. Instead they are propagated as actual errors: for `newPayload` they convert into
/// [`BeaconOnNewPayloadError`] and are returned as a JSON-RPC error object instead of a
/// `PayloadStatus` result, [`Self::InvalidParams`] as invalid params (`-32602`) and all other
/// variants as internal error (`-32603`).
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockFatalError {
    /// A provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// An internal or fatal block execution error.
    #[error(transparent)]
    BlockExecutionError(#[from] InternalBlockExecutionError),
    /// The payload params are malformed, e.g. undecodable block access list bytes, and the
    /// request must be rejected with an invalid params error.
    #[error(transparent)]
    InvalidParams(Box<dyn core::error::Error + Send + Sync>),
}

/// Error variants that are caused by invalid blocks.
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockValidationError {
    /// Block violated consensus rules.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),
    /// Validation error, transparently wrapping [`BlockValidationError`].
    #[error(transparent)]
    Validation(#[from] BlockValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;

    // Undecodable block access list bytes are malformed request params and must not be treated
    // as a block validation error.
    #[test]
    fn bal_decode_error_is_invalid_params() {
        let err = InsertBlockErrorKind::Consensus(ConsensusError::BlockAccessListDecode(
            alloy_rlp::Error::UnexpectedString,
        ));
        assert!(matches!(
            err.ensure_validation_error(),
            Err(InsertBlockFatalError::InvalidParams(_))
        ));
        assert!(matches!(
            BeaconOnNewPayloadError::from(InsertBlockFatalError::InvalidParams(Box::new(
                alloy_rlp::Error::UnexpectedString
            ))),
            BeaconOnNewPayloadError::InvalidParams(_)
        ));
    }
}
