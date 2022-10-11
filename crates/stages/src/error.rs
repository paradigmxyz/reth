use crate::pipeline::PipelineEvent;
use reth_db::kv::KVError;
use reth_primitives::BlockNumber;
use thiserror::Error;
use tokio::sync::mpsc::error::SendError;

/// A stage execution error.
#[derive(Error, Debug)]
pub enum StageError {
    /// The stage encountered a state validation error.
    ///
    /// TODO: This depends on the consensus engine and should include the validation failure reason
    #[error("Stage encountered a validation error in block {block}.")]
    Validation {
        /// The block that failed validation.
        block: BlockNumber,
    },
    /// The stage encountered a database error.
    #[error("A database error occurred.")]
    Database(#[from] KVError),
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// A pipeline execution error.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// The pipeline encountered an irrecoverable error in one of the stages.
    #[error("A stage encountered an irrecoverable error.")]
    Stage(#[from] StageError),
    /// The pipeline encountered a database error.
    #[error("A database error occurred.")]
    Database(#[from] KVError),
    /// The pipeline encountered an error while trying to send an event.
    #[error("The pipeline encountered an error while trying to send an event.")]
    Channel(#[from] SendError<PipelineEvent>),
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
