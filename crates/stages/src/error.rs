use crate::pipeline::PipelineEvent;
use reth_interfaces::{
    consensus, db::Error as DbError, executor, p2p::error::DownloadError, provider::ProviderError,
};
use reth_primitives::BlockNumber;
use reth_provider::TransactionError;
use thiserror::Error;
use tokio::sync::mpsc::error::SendError;

/// A stage execution error.
#[derive(Error, Debug)]
pub enum StageError {
    /// The stage encountered a state validation error.
    #[error("Stage encountered a validation error in block {block}: {error}.")]
    Validation {
        /// The block that failed validation.
        block: BlockNumber,
        /// The underlying consensus error.
        #[source]
        error: consensus::Error,
    },
    /// The stage encountered a database error.
    #[error("An internal database error occurred: {0}")]
    Database(#[from] DbError),
    #[error("Stage encountered a execution error in block {block}: {error}.")]
    /// The stage encountered a execution error
    // TODO: Probably redundant, should be rolled into `Validation`
    ExecutionError {
        /// The block that failed execution.
        block: BlockNumber,
        /// The underlying execution error.
        #[source]
        error: executor::Error,
    },
    /// Invalid checkpoint passed to the stage
    #[error("Invalid stage progress: {0}")]
    StageProgress(u64),
    /// Download channel closed
    #[error("Download channel closed")]
    ChannelClosed,
    /// The stage encountered a database integrity error.
    #[error("A database integrity error occurred: {0}")]
    DatabaseIntegrity(#[from] ProviderError),
    /// The stage encountered an error related to the current database transaction.
    #[error("A database transaction error occurred: {0}")]
    Transaction(#[from] TransactionError),
    /// Invalid download response. Applicable for stages which
    /// rely on external downloaders
    #[error("Invalid download response: {0}")]
    Download(#[from] DownloadError),
    /// The stage encountered a recoverable error.
    ///
    /// These types of errors are caught by the [Pipeline][crate::Pipeline] and trigger a restart
    /// of the stage.
    #[error(transparent)]
    Recoverable(Box<dyn std::error::Error + Send + Sync>),
    /// The stage encountered a fatal error.
    ///
    /// These types of errors stop the pipeline.
    #[error(transparent)]
    Fatal(Box<dyn std::error::Error + Send + Sync>),
}

impl StageError {
    /// If the error is fatal the pipeline will stop.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            StageError::Database(_) |
                StageError::Download(_) |
                StageError::DatabaseIntegrity(_) |
                StageError::StageProgress(_) |
                StageError::ExecutionError { .. } |
                StageError::ChannelClosed |
                StageError::Fatal(_)
        )
    }
}

/// A pipeline execution error.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// The pipeline encountered an irrecoverable error in one of the stages.
    #[error("A stage encountered an irrecoverable error.")]
    Stage(#[from] StageError),
    /// The pipeline encountered a database error.
    #[error("A database error occurred.")]
    Database(#[from] DbError),
    /// The pipeline encountered an error while trying to send an event.
    #[error("The pipeline encountered an error while trying to send an event.")]
    Channel(#[from] SendError<PipelineEvent>),
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
