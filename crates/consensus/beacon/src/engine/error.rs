use reth_payload_builder::error::PayloadBuilderError;
use reth_rpc_types::engine::{EngineRpcError, ForkchoiceUpdateError, PayloadError};
use reth_stages::PipelineError;
use thiserror::Error;

/// Beacon engine result.
pub type BeaconEngineResult<Ok> = Result<Ok, BeaconEngineError>;

/// The error wrapper for the beacon consensus engine.
#[derive(Error, Debug)]
pub enum BeaconEngineError {
    /// Forkchoice zero hash head received.
    #[error("Received zero hash as forkchoice head")]
    ForkchoiceEmptyHead,
    /// Pipeline channel closed.
    #[error("Pipeline channel closed")]
    PipelineChannelClosed,
    /// An error covered by the engine API standard error codes.
    #[error(transparent)]
    EngineApi(#[from] EngineRpcError),
    /// Encountered a payload error.
    #[error(transparent)]
    Payload(#[from] PayloadError),
    /// Encountered an error during the payload building process.
    #[error(transparent)]
    PayloadBuilderError(#[from] PayloadBuilderError),
    /// Pipeline error.
    #[error(transparent)]
    Pipeline(#[from] Box<PipelineError>),
    /// Common error. Wrapper around [reth_interfaces::Error].
    #[error(transparent)]
    Common(#[from] reth_interfaces::Error),
    /// Thrown when the engine task stopped
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
}

// box the pipeline error as it is a large enum.
impl From<PipelineError> for BeaconEngineError {
    fn from(e: PipelineError) -> Self {
        Self::Pipeline(Box::new(e))
    }
}

// for convenience in the beacon engine
impl From<reth_interfaces::db::Error> for BeaconEngineError {
    fn from(e: reth_interfaces::db::Error) -> Self {
        Self::Common(e.into())
    }
}

/// Represents error cases for an applied forkchoice update.
#[derive(Error, Debug, Eq, PartialEq)]
pub enum BeaconForkChoiceUpdateError {
    /// Thrown when a forkchoice update resulted in an error.
    #[error("Forkchoice update error: {0}")]
    ForkchoiceUpdateError(#[from] ForkchoiceUpdateError),
    /// Thrown when the engine task stopped
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
}
