use crate::pipeline::PipelineEvent;
use reth_interfaces::{consensus, db::Error as DbError, executor};
use reth_primitives::{BlockNumber, TxNumber, H256};
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
        matches!(
            self,
            StageError::Database(_) |
                StageError::DatabaseIntegrity(_) |
                StageError::StageProgress(_) |
                StageError::Fatal(_)
        )
    }
}

/// A database integrity error.
/// The sender stage error
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum DatabaseIntegrityError {
    /// The canonical header for a block is missing from the database.
    #[error("No canonical header for block #{number}")]
    CanonicalHeader {
        /// The block number key
        number: BlockNumber,
    },
    /// A header is missing from the database.
    #[error("No header for block #{number} ({hash})")]
    Header {
        /// The block number key
        number: BlockNumber,
        /// The block hash key
        hash: H256,
    },
    /// The cumulative transaction count is missing from the database.
    #[error("No cumulative tx count for #{number} ({hash})")]
    CumulativeTxCount {
        /// The block number key
        number: BlockNumber,
        /// The block hash key
        hash: H256,
    },
    /// A block body is missing.
    #[error("Block body not found for block #{number}")]
    BlockBody {
        /// The block number key
        number: BlockNumber,
    },
    #[error("Gap in transaction table. Missing tx number #{missing}.")]
    TransactionsGap { missing: TxNumber },
    #[error("Gap in transaction signer table. Missing tx number #{missing}.")]
    TransactionsSignerGap { missing: TxNumber },
    #[error("Got to the end of transaction table")]
    EndOfTransactionTable,
    #[error("Got to the end of the transaction sender table")]
    EndOfTransactionSenderTable,
    /// The total difficulty from the block header is missing.
    #[error("Total difficulty not found for block #{number}")]
    TotalDifficulty {
        /// The block number key
        number: BlockNumber,
    },
    /// The transaction is missing
    #[error("Transaction #{id} not found")]
    Transaction {
        /// The transaction id
        id: TxNumber,
    },
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
