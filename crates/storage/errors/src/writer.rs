use crate::db::DatabaseError;
use reth_primitives::StaticFileSegment;

/// `StorageWriter` related errors
#[derive(Clone, Debug, derive_more::Display, PartialEq, Eq)]
pub enum StorageWriterError {
    /// Database writer is missing
    #[display(fmt="Database writer is missing")]
    MissingDatabaseWriter,
    /// Static file writer is missing
    #[display(fmt="Static file writer is missing")]
    MissingStaticFileWriter,
    /// Static file writer is of wrong segment
    #[display(fmt="Static file writer is of wrong segment: got {_0}, expected {_1}")]
    IncorrectStaticFileWriter(StaticFileSegment, StaticFileSegment),
    /// Database-related errors.
    // #[error(transparent)]
    Database(DatabaseError),
}
