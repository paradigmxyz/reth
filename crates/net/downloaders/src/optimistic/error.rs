use reth_primitives::H256;
use thiserror::Error;

/// The downloader result.
pub type DownloaderResult<Ok> = std::result::Result<Ok, DownloaderError>;

/// The downloader error.
#[derive(Debug, Error)]
pub enum DownloaderError {
    /// The downloader is at capacity and cannot process new requests.
    #[error("Downloader is at maximum capacity.")]
    Full,
    /// The downloader received a body request, but header wasn't found.
    #[error("Missing header: {0}")]
    MissingHeader(H256),
}
