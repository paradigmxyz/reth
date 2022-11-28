use crate::pipeline::PipelineEvent;
use reth_interfaces::{consensus, db::Error as DbError};
use reth_primitives::{BlockNumber, H256};
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
    /// The stage encountered a database integrity error.
    #[error("A database integrity error occurred: {0}")]
    DatabaseIntegrity(#[from] DatabaseIntegrityError),
    /// Invalid download response. Applicable for stages which
    /// rely on external downloaders
    #[error("Invalid download response: {0}")]
    Download(String),
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// A database integrity error.
/// The sender stage error
#[derive(Error, Debug)]
pub enum DatabaseIntegrityError {
    // TODO(onbjerg): What's the difference between this and the one below?
    /// The canonical hash for a block is missing from the database.
    #[error("No canonical hash for block #{number}")]
    CanonicalHash {
        /// The block number key
        number: BlockNumber,
    },
    /// The canonical header for a block is missing from the database.
    #[error("No canonical hash for block #{number}")]
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
    #[error("No cumulative tx count for ${number} ({hash})")]
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
    /// The total difficulty from the block header is missing.
    #[error("Total difficulty not found for block #{number}")]
    TotalDifficulty {
        /// The block number key
        number: BlockNumber,
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
