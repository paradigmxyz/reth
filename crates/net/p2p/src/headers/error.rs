use derive_more::Display;
use reth_consensus::ConsensusError;
use reth_primitives::SealedHeader;

/// Header downloader result
pub type HeadersDownloaderResult<T> = Result<T, HeadersDownloaderError>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Clone, Eq, PartialEq, Display)]
pub enum HeadersDownloaderError {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    #[display(fmt = "valid downloaded header cannot be attached to the local head: {error}")]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: Box<SealedHeader>,
        /// The header we attempted to attach.
        header: Box<SealedHeader>,
        /// The error that occurred when attempting to attach the header.
        error: Box<ConsensusError>,
    },
}

#[cfg(feature = "std")]
impl std::error::Error for HeadersDownloaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DetachedHead { error, .. } => Some(error),
        }
    }
}
