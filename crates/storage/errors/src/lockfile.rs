use alloc::string::{String, ToString};
use core::fmt::{Display, Formatter, Result};
use reth_fs_util::FsPathError;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Storage lock error.
pub enum StorageLockError {
    /// Write lock taken
    Taken(usize),
    /// Indicates other unspecified errors.
    Other(String),
}

/// TODO: turn into variant once `ProviderError`
impl From<FsPathError> for StorageLockError {
    fn from(source: FsPathError) -> Self {
        Self::Other(source.to_string())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StorageLockError {}

impl Display for StorageLockError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Taken(write_lock) => write!(
                f,
                "storage directory is currently in use as read-write by another process: PID {}",
                write_lock
            ),
            Self::Other(unspecified) => write!(f, "{}", unspecified),
        }
    }
}
