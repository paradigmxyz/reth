use crate::db::DatabaseError;
use reth_primitives::StaticFileSegment;

/// `UnifiedStorageWriter` related errors
#[derive(Clone, Debug, thiserror_no_std::Error, PartialEq, Eq)]
pub enum UnifiedStorageWriterError {
    /// Static file writer is missing
    #[error("Static file writer is missing")]
    MissingStaticFileWriter,
    /// Static file writer is of wrong segment
    #[error("Static file writer is of wrong segment: got {0}, expected {1}")]
    IncorrectStaticFileWriter(StaticFileSegment, StaticFileSegment),
    /// Database-related errors.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}
