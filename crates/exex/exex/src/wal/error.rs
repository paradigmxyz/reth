//! Wal Errors

use std::path::PathBuf;

/// Wal Result type.
pub type WalResult<T> = Result<T, WalError>;

/// Wal Error types
#[derive(Debug, thiserror::Error)]
pub enum WalError {
    /// Filesystem error at the path
    #[error(transparent)]
    FsPathError(#[from] reth_fs_util::FsPathError),
    /// Directory entry reading error
    #[error("failed to get {0} directory entry: {1}")]
    DirEntry(PathBuf, #[source] std::io::Error),
    /// Error when reading file metadata
    #[error("failed to get metadata for file {0}: {1}")]
    FileMetadata(u32, #[source] std::io::Error),
    /// Parse error
    #[error("failed to parse file name: {0}")]
    Parse(String),
    /// Notification not found error
    #[error("notification {0} not found")]
    FileNotFound(u32),
    /// Decode error
    #[error("failed to decode notification {0} from {1}: {2}")]
    Decode(u32, PathBuf, #[source] rmp_serde::decode::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{error::Error, io};

    #[test]
    fn wrapped_errors_preserve_sources() {
        for error in [
            WalError::DirEntry(PathBuf::from("wal"), io::ErrorKind::PermissionDenied.into()),
            WalError::FileMetadata(1, io::ErrorKind::PermissionDenied.into()),
            WalError::Decode(
                1,
                PathBuf::from("wal/1.wal"),
                rmp_serde::decode::Error::InvalidDataRead(io::ErrorKind::PermissionDenied.into()),
            ),
        ] {
            let source = error.source().unwrap();
            let io_error = source
                .downcast_ref::<io::Error>()
                .or_else(|| {
                    source
                        .downcast_ref::<rmp_serde::decode::Error>()?
                        .source()?
                        .downcast_ref::<io::Error>()
                })
                .unwrap();
            assert_eq!(io_error.kind(), io::ErrorKind::PermissionDenied);
        }
    }
}
