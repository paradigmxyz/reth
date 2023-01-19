use reth_db::tx::DatabaseIntegrityError;
use reth_interfaces::{consensus, db::Error as DbError, executor};
use reth_primitives::BlockNumber;
use thiserror::Error;

impl From<DbError> for StageError {
    fn from(value: DbError) -> Self {
        StageError::DatabaseIntegrity(DatabaseIntegrityError::Inner(value))
    }
}

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
    /// The stage encountered a database integrity error.
    #[error("A database integrity error occurred: {0}")]
    DatabaseIntegrity(#[from] DatabaseIntegrityError),
    /// Invalid download response. Applicable for stages which
    /// rely on external downloaders
    #[error("Invalid download response: {0}")]
    Download(String),
    /// Invalid checkpoint passed to the stage
    #[error("Invalid stage progress: {0}")]
    StageProgress(u64),
    /// The stage encountered a recoverable error.
    ///
    /// These types of errors are caught by the [Pipeline] and trigger a restart of the stage.
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
        matches!(self, |StageError::DatabaseIntegrity(_)| StageError::StageProgress(_) |
            StageError::Fatal(_))
    }
}
