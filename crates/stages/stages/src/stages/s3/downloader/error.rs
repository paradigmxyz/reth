use alloy_primitives::B256;
use reth_fs_util::FsPathError;

/// Possible downloader error variants.
#[derive(Debug, thiserror::Error)]
pub enum DownloaderError {
    /// Requires a valid `total_size` {0}
    #[error("requires a valid total_size")]
    InvalidMetadataTotalSize(Option<usize>),
    #[error("tried to access chunk on index {0}, but there's only {1} chunks")]
    /// Invalid chunk access
    InvalidChunk(usize, usize),
    // File hash mismatch.
    #[error("file hash does not match the expected one {0} != {1} ")]
    InvalidFileHash(B256, B256),
    // Empty content length returned from the server.
    #[error("metadata got an empty content length from server")]
    EmptyContentLength,
    /// Reqwest error
    #[error(transparent)]
    FsPath(#[from] FsPathError),
    /// Reqwest error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    /// Std Io error
    #[error(transparent)]
    StdIo(#[from] std::io::Error),
    /// Bincode error
    #[error(transparent)]
    Bincode(#[from] bincode::Error),
}
