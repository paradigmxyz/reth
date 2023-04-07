use reth_miner::error::PayloadError as BuilderError;
use reth_rpc_types::engine::PayloadError;
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
    /// Invalid payload attributes.
    #[error("Invalid payload attributes")]
    InvalidPayloadAttributes,
    /// Pipeline channel closed.
    #[error("Pipeline channel closed")]
    PipelineChannelClosed,
    /// Unknown payload
    #[error("Unknown payload")]
    UnknownPayload,
    /// Encountered a payload error.
    #[error(transparent)]
    Payload(#[from] PayloadError),
    /// Encountered an error during the payload building process.
    #[error(transparent)]
    BuilderError(#[from] BuilderError),
    /// Pipeline error.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    /// Common error. Wrapper around [reth_interfaces::Error].
    #[error(transparent)]
    Common(#[from] reth_interfaces::Error),
}

// for convenience in the beacon engine
impl From<reth_interfaces::db::Error> for BeaconEngineError {
    fn from(e: reth_interfaces::db::Error) -> Self {
        Self::Common(e.into())
    }
}
