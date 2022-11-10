use crate::pipeline::PipelineEvent;
use reth_interfaces::db::Error as DbError;
use reth_primitives::{BlockNumber, H256};
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
    #[error("An internal database error occurred.")]
    Database(#[from] DbError),
    /// The stage encountered a database integrity error.
    #[error("A database integrity error occurred.")]
    DatabaseIntegrity(#[from] DatabaseIntegrityError),
    /// The stage encountered an internal error.
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// A database integrity error.
/// The sender stage error
#[derive(Error, Debug)]
pub enum DatabaseIntegrityError {
    /// Cannonical hash is missing from db
    #[error("no cannonical hash for block #{number}")]
    CannonicalHash {
        /// The block number key
        number: BlockNumber,
    },
    /// Cannonical header is missing from db
    #[error("no cannonical hash for block #{number}")]
    CannonicalHeader {
        /// The block number key
        number: BlockNumber,
    },
    /// Header is missing from db
    #[error("no header for block #{number} ({hash})")]
    Header {
        /// The block number key
        number: BlockNumber,
        /// The block hash key
        hash: H256,
    },
    /// Cumulative transaction count is missing from db
    #[error("no cumulative tx count for ${number} ({hash})")]
    CumulativeTxCount {
        /// The block number key
        number: BlockNumber,
        /// The block hash key
        hash: H256,
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
