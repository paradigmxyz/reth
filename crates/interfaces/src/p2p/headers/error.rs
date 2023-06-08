use crate::consensus::ConsensusError;
use reth_primitives::{BlockHash, BlockNumber};
use thiserror::Error;

/// Header downloader result
pub type HeadersDownloaderResult<T> = Result<T, HeadersDownloaderError>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum HeadersDownloaderError {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    #[error("Header cannot be attached. Details: {error}.")]
    HeaderCannotBeAttached {
        /// The number of the header that we attempted to attach.
        number: BlockNumber,
        /// The hash of the header that we attempted to attach.
        hash: BlockHash,
        /// The error that occurred when attempting to attach the header.
        error: ConsensusError,
    },
}
