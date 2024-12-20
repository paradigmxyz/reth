use alloy_primitives::B256;
use reth_fs_util::FsPathError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DownloaderError {
    /// Requires a valid `total_size` {0}
    #[error("requires a valid total_size")]
    InvalidMetadataTotalSize(Option<usize>),
    #[error("Tried to access chunk on index {0}, but there's only {1} chunks")]
    /// Invalid chunk access
    InvalidChunk(usize, usize),
    #[error("url requires content length and range requests")]
    InvalidUrl,
    #[error("file hash does not match the expected one {0} != {1} ")]
    InvalidFileHash(B256, B256),
    #[error(transparent)]
    FsPath(#[from] FsPathError),
    /// Reqwest error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    /// Std io error
    #[error(transparent)]
    StdIo(#[from] std::io::Error),
}
