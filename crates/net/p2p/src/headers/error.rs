use derive_more::Display;
use reth_consensus::ConsensusError;
use reth_primitives::alloy_primitives::Sealed;

/// Header downloader result
pub type HeadersDownloaderResult<H> = Result<Vec<Sealed<H>>, HeadersDownloaderError<H>>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Clone, Eq, PartialEq, Display)]
pub enum HeadersDownloaderError<H> {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    #[display("valid downloaded header cannot be attached to the local head: {error}")]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: Box<Sealed<H>>,
        /// The header we attempted to attach.
        header: Box<Sealed<H>>,
        /// The error that occurred when attempting to attach the header.
        error: Box<ConsensusError>,
    },
}

#[cfg(feature = "std")]
impl<H: std::fmt::Debug> std::error::Error for HeadersDownloaderError<H> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DetachedHead { error, .. } => Some(error),
        }
    }
}
