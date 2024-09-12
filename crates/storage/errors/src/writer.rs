use crate::db::DatabaseError;
use reth_primitives::StaticFileSegment;

/// `UnifiedStorageWriter` related errors
/// `StorageWriter` related errors
#[derive(Clone, Debug, derive_more::Display, PartialEq, Eq, derive_more::Error)]
pub enum UnifiedStorageWriterError {
    /// Database writer is missing
    #[display("Database writer is missing")]
    MissingDatabaseWriter,
    /// Static file writer is missing
    #[display("Static file writer is missing")]
    MissingStaticFileWriter,
    /// Static file writer is of wrong segment
    #[display("Static file writer is of wrong segment: got {_0}, expected {_1}")]
    IncorrectStaticFileWriter(StaticFileSegment, StaticFileSegment),
    /// Database-related errors.
    Database(DatabaseError),
}

impl From<DatabaseError> for UnifiedStorageWriterError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}
