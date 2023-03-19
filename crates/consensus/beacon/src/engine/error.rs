use reth_stages::PipelineError;
use thiserror::Error;

/// The error wrapper for the beacon consensus engine.
#[derive(Error, Debug)]
pub enum BeaconEngineError {
    /// Pipeline error.
    #[error("Controller encountered a pipeline error.")]
    Pipeline(#[from] PipelineError),
    /// Database error.
    #[error("Controller encountered a database error.")]
    Database(#[from] reth_db::Error),
    /// Common error. Wrapper around [reth_interfaces::Error].
    #[error("Controller encountered an error.")]
    Common(#[from] reth_interfaces::Error),
}
