use crate::consensus;
use reth_primitives::{rpc::BlockNumber, WithPeerId, H256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Result alias for result of a request.
pub type RequestResult<T> = Result<T, RequestError>;

/// Result with [PeerId]
pub type PeerRequestResult<T> = RequestResult<WithPeerId<T>>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum RequestError {
    #[error("Closed channel to the peer.")]
    ChannelClosed,
    #[error("Not connected to the peer.")]
    NotConnected,
    #[error("Connection to a peer dropped while handling the request.")]
    ConnectionDropped,
    #[error("Capability Message is not supported by remote peer.")]
    UnsupportedCapability,
    #[error("Request timed out while awaiting response.")]
    Timeout,
    #[error("Received bad response.")]
    BadResponse,
}

// === impl RequestError ===

impl RequestError {
    /// Indicates whether this error is retryable or fatal.
    pub fn is_retryable(&self) -> bool {
        matches!(self, RequestError::Timeout | RequestError::ConnectionDropped)
    }

    /// Whether the error happened because the channel was closed.
    pub fn is_channel_closed(&self) -> bool {
        matches!(self, RequestError::ChannelClosed)
    }
}

impl<T> From<mpsc::error::SendError<T>> for RequestError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        RequestError::ChannelClosed
    }
}

impl From<oneshot::error::RecvError> for RequestError {
    fn from(_: oneshot::error::RecvError) -> Self {
        RequestError::ChannelClosed
    }
}

/// The download result type
pub type DownloadResult<T> = Result<T, DownloadError>;

/// The downloader error type
#[derive(Error, Debug, Clone)]
pub enum DownloadError {
    /// Header validation failed
    #[error("Failed to validate header {hash}. Details: {error}.")]
    HeaderValidation {
        /// Hash of header failing validation
        hash: H256,
        /// The details of validation failure
        #[source]
        error: consensus::Error,
    },
    /// Block validation failed
    #[error("Failed to validate body for header {hash}. Details: {error}.")]
    BlockValidation {
        /// Hash of header failing validation
        hash: H256,
        /// The details of validation failure
        #[source]
        error: consensus::Error,
    },
    /// Timed out while waiting for request id response.
    #[error("Timed out while getting headers for request.")]
    Timeout,
    /// Error when checking that the current [`Header`] has the parent's hash as the parent_hash
    /// field, and that they have sequential block numbers.
    #[error("Headers did not match, current number: {header_number} / current hash: {header_hash}, parent number: {parent_number} / parent_hash: {parent_hash}")]
    MismatchedHeaders {
        /// The header number being evaluated
        header_number: BlockNumber,
        /// The header hash being evaluated
        header_hash: H256,
        /// The parent number being evaluated
        parent_number: BlockNumber,
        /// The parent hash being evaluated
        parent_hash: H256,
    },
    /// Received empty response while expecting headers
    #[error("Received empty header response.")]
    EmptyResponse,
    /// Received an invalid tip
    #[error("Received invalid tip: {received:?}. Expected {expected:?}.")]
    InvalidTip {
        /// The hash of the received tip
        received: H256,
        /// The hash of the expected tip
        expected: H256,
    },
    /// Received a response to a request with unexpected start block
    #[error("Headers response starts at unexpected block: {received:?}. Expected {expected:?}.")]
    HeadersResponseStartBlockMismatch {
        /// The block number of the received tip
        received: u64,
        /// The hash of the expected tip
        expected: u64,
    },
    /// Received headers with less than expected items.
    #[error("Received less headers than expected: {received:?}. Expected {expected:?}.")]
    HeadersResponseTooShort {
        /// How many headers we received.
        received: u64,
        /// How many headers we expected.
        expected: u64,
    },
    /// Error while executing the request.
    #[error(transparent)]
    RequestError(#[from] RequestError),
}
