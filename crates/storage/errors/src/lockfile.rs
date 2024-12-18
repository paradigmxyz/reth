use alloc::string::{String, ToString};
use reth_fs_util::FsPathError;

/// Storage lock error.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Display)]
pub enum StorageLockError {
    /// Write lock taken
    #[display("storage directory is currently in use as read-write by another process: PID {_0}")]
    Taken(usize),
    /// Indicates other unspecified errors.
    #[display("{_0}")]
    Other(String),
}

impl core::error::Error for StorageLockError {}

/// TODO: turn into variant once `ProviderError`
impl From<FsPathError> for StorageLockError {
    fn from(error: FsPathError) -> Self {
        Self::Other(error.to_string())
    }
}
