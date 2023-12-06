use crate::engine::hooks::EngineHookError;
use reth_interfaces::RethError;
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
    /// Common error. Wrapper around [RethError].
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
impl From<reth_interfaces::db::DatabaseError> for BeaconConsensusEngineError {
    fn from(e: reth_interfaces::db::DatabaseError) -> Self {
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
    /// Internal errors, for example, error while reading from the database.
    #[error(transparent)]
    Internal(Box<RethError>),
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
}

impl From<RethError> for BeaconForkChoiceUpdateError {
    fn from(e: RethError) -> Self {
        Self::Internal(Box::new(e))
    }
}
impl From<reth_interfaces::db::DatabaseError> for BeaconForkChoiceUpdateError {
    fn from(e: reth_interfaces::db::DatabaseError) -> Self {
        Self::Internal(Box::new(e.into()))
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
    /// Thrown when a block has blob transactions, but is not after the Cancun fork.
    #[error("block has blob transactions, but is not after the Cancun fork")]
    PreCancunBlockWithBlobTransactions,
    /// An internal error occurred, not necessarily related to the payload.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
