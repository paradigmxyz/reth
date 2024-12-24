use derive_more::{Display, Error};
use reth_consensus::ConsensusError;
use reth_primitives::SealedHeader;

/// Header downloader result
pub type HeadersDownloaderResult<T, H> = Result<T, HeadersDownloaderError<H>>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Clone, Eq, PartialEq, Display, Error)]
pub enum HeadersDownloaderError<H> {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    #[display("valid downloaded header cannot be attached to the local head: {error}")]
    DetachedHead {
        /// The local head we attempted to attach to.
        local_head: Box<SealedHeader<H>>,
        /// The header we attempted to attach.
        header: Box<SealedHeader<H>>,
        /// The error that occurred when attempting to attach the header.
        #[error(source)]
        error: Box<ConsensusError>,
    },
}
