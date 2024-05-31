use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
/// Storage lock error.
pub enum StorageLockError {
    /// Write lock taken
    #[error("storage directory is currently in use as read-write by another process: {0}")]
    Taken(usize),
    /// Indicates other unspecified errors.
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for StorageLockError {
    fn from(source: std::io::Error) -> Self {
        Self::Other(source.to_string())
    }
}
