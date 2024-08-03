use reth_fs_util::FsPathError;

#[cfg(not(feature = "std"))]
use alloc::string::{String, ToString};

/// Storage lock error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror_no_std::Error)]
pub enum StorageLockError {
    /// Write lock taken
    #[error("storage directory is currently in use as read-write by another process: PID {0}")]
    Taken(usize),
    /// Indicates other unspecified errors.
    #[error("{0}")]
    Other(String),
}

/// TODO: turn into variant once `ProviderError`
impl From<FsPathError> for StorageLockError {
    fn from(source: FsPathError) -> Self {
        Self::Other(source.to_string())
    }
}
