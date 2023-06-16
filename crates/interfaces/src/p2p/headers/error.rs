use crate::consensus::ConsensusError;
use reth_primitives::SealedHeader;
use thiserror::Error;

/// Header downloader result
pub type HeadersDownloaderResult<T> = Result<T, HeadersDownloaderError>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum HeadersDownloaderError {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    #[error("Valid downloaded header cannot be attached to the local head. Details: {error}.")]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: SealedHeader,
        /// The header we attempted to attach.
        header: SealedHeader,
        /// The error that occurred when attempting to attach the header.
        error: Box<ConsensusError>,
    },
}
