use crate::engine::hooks::EngineHookError;
pub use reth_engine_primitives::BeaconForkChoiceUpdateError;
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
