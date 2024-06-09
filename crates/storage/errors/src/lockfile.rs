use core::fmt::{Display, Formatter, Result};
use reth_fs_util::FsPathError;

#[cfg(feature = "std")]
use std::error::Error;

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
impl Error for StorageLockError {}

impl Display for StorageLockError {
    fn fmt(&self, __formatter: &mut Formatter<'_>) -> Result {
        use thiserror::__private::AsDisplay as _;
        match self {
            Self::Taken(write_lock) => __formatter.write_fmt(format_args!(
                "storage directory is currently in use as read-write by another process: PID {0}",
                write_lock.as_display(),
            )),
            Self::Other(unspecified) => {
                __formatter.write_fmt(format_args!("{0}", unspecified.as_display()))
            }
        }
    }
}
