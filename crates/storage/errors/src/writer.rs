use crate::db::DatabaseError;
use reth_primitives::StaticFileSegment;

/// `UnifiedStorageWriter` related errors
/// `StorageWriter` related errors
#[derive(Clone, Debug, derive_more::Display, PartialEq, Eq)]
pub enum UnifiedStorageWriterError {
    /// Database writer is missing
    #[display(fmt = "Database writer is missing")]
    MissingDatabaseWriter,
    /// Static file writer is missing
    #[display(fmt = "Static file writer is missing")]
    MissingStaticFileWriter,
    /// Static file writer is of wrong segment
    #[display(fmt = "Static file writer is of wrong segment: got {_0}, expected {_1}")]
    IncorrectStaticFileWriter(StaticFileSegment, StaticFileSegment),
    /// Database-related errors.
    Database(DatabaseError),
}

#[cfg(feature = "std")]
impl std::error::Error for UnifiedStorageWriterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(source) => std::error::Error::source(source),
            _ => Option::None,
        }
    }
}

impl From<DatabaseError> for UnifiedStorageWriterError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}
