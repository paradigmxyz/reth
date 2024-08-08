use reth_consensus::ConsensusError;
use reth_primitives::alloy_primitives::Sealed;
use thiserror::Error;

/// Header downloader result
pub type HeadersDownloaderResult<T, H> = Result<T, HeadersDownloaderError<H>>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum HeadersDownloaderError<H> {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    #[error("valid downloaded header cannot be attached to the local head: {error}")]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: Box<Sealed<H>>,
        /// The header we attempted to attach.
        header: Box<Sealed<H>>,
        /// The error that occurred when attempting to attach the header.
        #[source]
        error: Box<ConsensusError>,
    },
}
