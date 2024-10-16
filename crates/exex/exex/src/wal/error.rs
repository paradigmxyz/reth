//! Wal Errors

use std::path::PathBuf;

/// Wal Result type.
pub type WalResult<T> = Result<T, WalError>;

/// Wal Error types
#[derive(Debug, thiserror::Error)]
pub enum WalError {
    /// Directory operation `FsPathError`
    #[error("directory operation error: {0}")]
    DirFsPath(reth_fs_util::FsPathError),
    /// Directory operation IO Error
    #[error("directory operation error: {0}")]
    DirIO(std::io::Error),
    /// File operation `FsPathError`
    #[error("file operation error on file with id {0}: {1}")]
    FileFsPath(u32, reth_fs_util::FsPathError),
    /// File operation IO Error
    #[error("file operation error on file with id {0}: {1}")]
    FileIO(u32, std::io::Error),
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
