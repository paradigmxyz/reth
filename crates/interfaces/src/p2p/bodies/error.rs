use crate::p2p::error::RequestError;
use reth_primitives::H256;
use thiserror::Error;

/// Body client errors.
#[derive(Error, Debug, Clone)]
pub enum BodiesClientError {
    /// Timed out while waiting for a response.
    #[error("Timed out while getting bodies for block {header_hash}.")]
    Timeout {
        /// The header hash of the block that timed out.
        header_hash: H256,
    },
    /// The client encountered an internal error.
    #[error(transparent)]
    Internal(#[from] RequestError),
}

/// Body downloader errors.
#[derive(Error, Debug, Clone)]
pub enum DownloadError {
    /// Timed out while waiting for a response.
    #[error("Timed out while getting bodies for block {header_hash}.")]
    Timeout {
        /// The header hash of the block that timed out.
        header_hash: H256,
    },
    /// The [BodiesClient] used by the downloader experienced an error.
    #[error("The downloader client encountered an error.")]
    Client {
        /// The underlying client error.
        #[source]
        source: BodiesClientError,
    },
}

impl From<BodiesClientError> for DownloadError {
    fn from(error: BodiesClientError) -> Self {
        match error {
            BodiesClientError::Timeout { header_hash } => DownloadError::Timeout { header_hash },
            _ => DownloadError::Client { source: error },
        }
    }
}

impl DownloadError {
    /// Indicates whether this error is retryable or fatal.
    pub fn is_retryable(&self) -> bool {
        matches!(self, DownloadError::Timeout { .. })
    }
}
