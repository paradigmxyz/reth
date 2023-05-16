use reth_rpc_types::engine::ForkchoiceUpdateError;
use reth_stages::PipelineError;

/// Beacon engine result.
pub type BeaconEngineResult<Ok> = Result<Ok, BeaconConsensusEngineError>;

/// The error type for the beacon consensus engine service
/// [BeaconConsensusEngine](crate::BeaconConsensusEngine)
///
/// Represents all possible error cases for the beacon consensus engine.
#[derive(Debug, thiserror::Error)]
pub enum BeaconConsensusEngineError {
    /// Pipeline channel closed.
    #[error("Pipeline channel closed")]
    PipelineChannelClosed,
    /// Pipeline error.
    #[error(transparent)]
    Pipeline(#[from] Box<PipelineError>),
    /// Common error. Wrapper around [reth_interfaces::Error].
    #[error(transparent)]
    Common(#[from] reth_interfaces::Error),
}

// box the pipeline error as it is a large enum.
impl From<PipelineError> for BeaconConsensusEngineError {
    fn from(e: PipelineError) -> Self {
        Self::Pipeline(Box::new(e))
    }
}

// for convenience in the beacon engine
impl From<reth_interfaces::db::Error> for BeaconConsensusEngineError {
    fn from(e: reth_interfaces::db::Error) -> Self {
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
    #[error("Forkchoice update error: {0}")]
    ForkchoiceUpdateError(#[from] ForkchoiceUpdateError),
    /// Internal errors, for example, error while reading from the database.
    #[error(transparent)]
    Internal(Box<reth_interfaces::Error>),
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
}

impl From<reth_interfaces::Error> for BeaconForkChoiceUpdateError {
    fn from(e: reth_interfaces::Error) -> Self {
        Self::Internal(Box::new(e))
    }
}
impl From<reth_interfaces::db::Error> for BeaconForkChoiceUpdateError {
    fn from(e: reth_interfaces::db::Error) -> Self {
        Self::Internal(Box::new(e.into()))
    }
}

/// Represents all error cases when handling a new payload.
///
/// This represents all possible error cases that must be returned as JSON RCP errors back to the
/// beacon node.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum BeaconOnNewPayloadError {
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
}
