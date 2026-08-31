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
        Self::internal(error)
    }
}

impl From<InsertBlockProcessingError> for BeaconOnNewPayloadError {
    fn from(error: InsertBlockProcessingError) -> Self {
        match error {
            InsertBlockProcessingError::MalformedInput(error) => Self::InvalidParams(error),
            InsertBlockProcessingError::Fatal(error) => Self::internal(error),
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
    /// Supplemental block access list bytes could not be decoded.
    #[error(transparent)]
    BlockAccessListDecode(#[from] BlockAccessListDecodeError),
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

    /// Returns an [`InsertBlockValidationError`] if the failure invalidates the block, or an
    /// [`InsertBlockProcessingError`] if the failure is not attributable to the block itself.
    ///
    /// This distinction controls whether the block may be returned as `INVALID` and its hash cached
    /// as invalid. Because `INVALID` has consensus meaning, malformed supplemental input and
    /// internal failures must remain processing errors instead.
    pub fn ensure_validation_error(
        self,
    ) -> Result<InsertBlockValidationError, InsertBlockProcessingError> {
        match self {
            Self::BlockAccessListDecode(error) => {
                Err(InsertBlockProcessingError::MalformedInput(Box::new(error)))
            }
            Self::Consensus(err) => Ok(InsertBlockValidationError::Consensus(err)),
            Self::Execution(err) => match err {
                BlockExecutionError::Validation(err) => {
                    Ok(InsertBlockValidationError::Validation(err))
                }
                BlockExecutionError::Internal(error) => Err(InsertBlockProcessingError::Fatal(
                    InsertBlockFatalError::BlockExecutionError(error),
                )),
            },
            Self::Provider(err) => {
                Err(InsertBlockProcessingError::Fatal(InsertBlockFatalError::Provider(err)))
            }
            Self::Other(err) => Err(InsertBlockProcessingError::Fatal(
                InternalBlockExecutionError::Other(err).into(),
            )),
        }
    }
}

/// Error decoding supplemental block access list bytes.
#[derive(Debug, thiserror::Error)]
#[error("failed to decode block access list: {0}")]
pub struct BlockAccessListDecodeError(#[source] Box<dyn core::error::Error + Send + Sync>);

impl BlockAccessListDecodeError {
    /// Creates a new block access list decode error.
    pub fn new<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self(Box::new(error))
    }
}

/// An error that occurs while processing a block but does not invalidate the block itself.
///
/// These errors must not produce an `INVALID`
/// [`PayloadStatus`](alloy_rpc_types_engine::PayloadStatus) or cause the block hash to be cached as
/// invalid. Malformed supplemental input remains distinct from fatal processing failures so
/// `newPayload` can reject it as invalid params, while internal ingestion paths can discard it
/// without terminating the engine task.
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockProcessingError {
    /// Supplemental input is malformed, but the block itself is not known to be invalid.
    #[error(transparent)]
    MalformedInput(Box<dyn core::error::Error + Send + Sync>),
    /// Block processing cannot continue because of an internal failure.
    #[error(transparent)]
    Fatal(#[from] InsertBlockFatalError),
}

/// Internal errors that prevent block processing from continuing.
#[derive(Debug, thiserror::Error)]
pub enum InsertBlockFatalError {
    /// A provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// An internal or fatal block execution error.
    #[error(transparent)]
    BlockExecutionError(#[from] InternalBlockExecutionError),
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
    fn ensure_insert_block_validation_error() {
        let err = InsertBlockErrorKind::BlockAccessListDecode(BlockAccessListDecodeError::new(
            alloy_rlp::Error::UnexpectedString,
        ));
        assert!(matches!(
            err.ensure_validation_error(),
            Err(InsertBlockProcessingError::MalformedInput(_))
        ));

        assert!(matches!(
            InsertBlockErrorKind::Consensus(ConsensusError::BlockAccessListHashMissing)
                .ensure_validation_error(),
            Ok(InsertBlockValidationError::Consensus(_))
        ));

        assert!(matches!(
            InsertBlockErrorKind::Provider(ProviderError::BestBlockNotFound)
                .ensure_validation_error(),
            Err(InsertBlockProcessingError::Fatal(InsertBlockFatalError::Provider(_)))
        ));
    }
}
