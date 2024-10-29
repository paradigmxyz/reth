use crate::engine::hooks::EngineHookError;
use alloy_rpc_types_engine::ForkchoiceUpdateError;
use reth_errors::{DatabaseError, RethError};
use reth_stages_api::PipelineError;

/// Beacon engine result.
pub type BeaconEngineResult<Ok> = Result<Ok, BeaconConsensusEngineError>;

/// The error type for the beacon consensus engine service
/// [`BeaconConsensusEngine`](crate::BeaconConsensusEngine)
///
/// Represents all possible error cases for the beacon consensus engine.
#[derive(Debug, thiserror::Error)]
pub enum BeaconConsensusEngineError {
    /// Pipeline channel closed.
    #[error("pipeline channel closed")]
    PipelineChannelClosed,
    /// Pipeline error.
    #[error(transparent)]
    Pipeline(#[from] Box<PipelineError>),
    /// Pruner channel closed.
    #[error("pruner channel closed")]
    PrunerChannelClosed,
    /// Hook error.
    #[error(transparent)]
    Hook(#[from] EngineHookError),
    /// Common error. Wrapper around [`RethError`].
    #[error(transparent)]
    Common(#[from] RethError),
}

// box the pipeline error as it is a large enum.
impl From<PipelineError> for BeaconConsensusEngineError {
    fn from(e: PipelineError) -> Self {
        Self::Pipeline(Box::new(e))
    }
}

// for convenience in the beacon engine
impl From<DatabaseError> for BeaconConsensusEngineError {
    fn from(e: DatabaseError) -> Self {
        Self::Common(e.into())
    }
}

/// Represents error cases for an applied forkchoice update.
///
/// This represents all possible error cases, that must be returned as JSON RCP errors back to the
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

impl From<RethError> for BeaconForkChoiceUpdateError {
    fn from(e: RethError) -> Self {
        Self::internal(e)
    }
}
impl From<DatabaseError> for BeaconForkChoiceUpdateError {
    fn from(e: DatabaseError) -> Self {
        Self::internal(e)
    }
}

/// Represents all error cases when handling a new payload.
///
/// This represents all possible error cases that must be returned as JSON RCP errors back to the
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
