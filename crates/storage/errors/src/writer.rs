use crate::db::DatabaseError;

/// `StorageWriter` related errors
#[derive(Clone, Debug, thiserror_no_std::Error, PartialEq, Eq)]
pub enum StorageWriterError {
    /// Database writer is missing
    #[error("Database writer is missing")]
    MissingDatabaseWriter,
    /// Static file writer is missing
    #[error("Static file writer is missing")]
    MissingStaticFileWriter,
    /// Database-related errors.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}
