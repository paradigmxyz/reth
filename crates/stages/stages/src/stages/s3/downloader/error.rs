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
    /// Reqwest error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    /// Std io error
    #[error(transparent)]
    StdIo(#[from] std::io::Error),
}
