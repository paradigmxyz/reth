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
    /// Encountered a payload error.
    #[error(transparent)]
    Payload(#[from] PayloadError),
    /// Pipeline error.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    /// Common error. Wrapper around [reth_interfaces::Error].
    #[error(transparent)]
    Common(#[from] reth_interfaces::Error),
}
