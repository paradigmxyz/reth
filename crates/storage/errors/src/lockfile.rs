use reth_fs_util::FsPathError;

#[cfg(not(feature = "std"))]
use alloc::string::{String, ToString};

use crate::provider::ProviderError;

/// Storage lock error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror_no_std::Error)]
pub enum StorageLockError {
    /// Write lock taken
    #[error("storage directory is currently in use as read-write by another process: PID {0}")]
    Taken(usize),
    /// Indicates other unspecified errors.
    #[error("{0}")]
    Other(String),
    /// Represents an error from the Provider
    #[error("provider error: {0}")]
    ProviderError(Box<ProviderError>),
}

impl From<FsPathError> for StorageLockError {
    fn from(source: FsPathError) -> Self {
        Self::Other(source.to_string())
    }
}

impl From<ProviderError> for StorageLockError {
    fn from(source: ProviderError) -> Self {
        Self::ProviderError(Box::new(source))
    }
}