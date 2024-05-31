use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
/// Storage lock error.
pub enum StorageLockError {
    /// Write lock taken
    #[error("write lock already taken")]
    Taken,
    /// Indicates other unspecified errors.
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for StorageLockError {
    fn from(source: std::io::Error) -> Self {
        Self::Other(source.to_string())
    }
}