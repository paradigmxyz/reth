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
    DirEntry(PathBuf, std::io::Error),
    /// Error when reading file metadata
    #[error("failed to get metadata for file {0}: {1}")]
    FileMetadata(u32, std::io::Error),
    /// Parse error
    #[error("failed to parse file name: {0}")]
    Parse(String),
    /// Notification not found error
    #[error("notification {0} not found")]
    FileNotFound(u32),
    /// Decode error
    #[error("failed to decode notification {0} from {1}: {2}")]
    Decode(u32, PathBuf, rmp_serde::decode::Error),
}
