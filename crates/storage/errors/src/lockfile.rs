use reth_fs_util::FsPathError;

#[cfg(not(feature = "std"))]
use alloc::string::{String, ToString};

/// Storage lock error.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Display)]
pub enum StorageLockError {
    /// Write lock taken
    #[display(fmt = "storage directory is currently in use as read-write by another process: PID {_0}")]
    Taken(usize),
    /// Indicates other unspecified errors.
    #[display(fmt = "{_0}")]
    Other(String),
}

/// TODO: turn into variant once `ProviderError`
impl From<FsPathError> for StorageLockError {
    fn from(source: FsPathError) -> Self {
        Self::Other(source.to_string())
    }
}
