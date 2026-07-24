use alloc::boxed::Box;
use alloy_primitives::BlockNumber;
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

/// Represents error cases when setting the canonical head through the debug API.
#[derive(Debug, thiserror::Error)]
pub enum DebugSetHeadError {
    /// Thrown when the engine task is unavailable/stopped.
    #[error("consensus engine task stopped")]
    EngineUnavailable,
    /// The requested canonical block does not exist.
    #[error("block {0} not found")]
    BlockNotFound(BlockNumber),
    /// The requested block is below the finalized block.
    #[error("cannot set head to block {target} below finalized block {finalized}")]
    Finalized {
        /// The requested block number.
        target: BlockNumber,
        /// The current finalized block number.
        finalized: BlockNumber,
    },
    /// The pipeline is actively syncing and owns canonical chain progress.
    #[error("cannot set head while pipeline sync is active")]
    Syncing,
    /// An internal error occurred while updating the canonical head.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl DebugSetHeadError {
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
    /// Returns an [`InsertBlockFatalError`] if the error is caused by an error that is not
    /// validation related or is otherwise fatal.
    ///
    /// This is intended to be used to determine if we should respond `INVALID` as a response when
    /// processing a new block.
    pub fn ensure_validation_error(
        self,
    ) -> Result<InsertBlockValidationError, InsertBlockFatalError> {
        match self {
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
