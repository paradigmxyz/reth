use std::ops::RangeInclusive;

use reth_consensus::ConsensusError;
use reth_network_peers::WithPeerId;
use reth_primitives::{
    BlockHashOrNumber, BlockNumber, GotExpected, GotExpectedBoxed, Header, B256,
};
use reth_storage_errors::{db::DatabaseError, provider::ProviderError};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use super::headers::client::HeadersRequest;
use crate::ReputationChangeKind;

/// Result alias for result of a request.
pub type RequestResult<T> = Result<T, RequestError>;

/// Result with [`PeerId`][reth_network_peers::PeerId]
pub type PeerRequestResult<T> = RequestResult<WithPeerId<T>>;

/// Helper trait used to validate responses.
pub trait EthResponseValidator {
    /// Determine whether the response matches what we requested in [`HeadersRequest`]
    fn is_likely_bad_headers_response(&self, request: &HeadersRequest) -> bool;

    /// Return the response reputation impact if any
    fn reputation_change_err(&self) -> Option<ReputationChangeKind>;
}

impl EthResponseValidator for RequestResult<Vec<Header>> {
    fn is_likely_bad_headers_response(&self, request: &HeadersRequest) -> bool {
        match self {
            Ok(headers) => {
                let request_length = headers.len() as u64;

                if request_length <= 1 && request.limit != request_length {
                    return true
                }

                match request.start {
                    BlockHashOrNumber::Number(block_number) => headers
                        .first()
                        .map(|header| block_number != header.number)
                        .unwrap_or_default(),
                    BlockHashOrNumber::Hash(_) => {
                        // we don't want to hash the header
                        false
                    }
                }
            }
            Err(_) => true,
        }
    }

    /// [`RequestError::ChannelClosed`] is not possible here since these errors are mapped to
    /// `ConnectionDropped`, which will be handled when the dropped connection is cleaned up.
    ///
    /// [`RequestError::ConnectionDropped`] should be ignored here because this is already handled
    /// when the dropped connection is handled.
    ///
    /// [`RequestError::UnsupportedCapability`] is not used yet because we only support active
    /// session for eth protocol.
    fn reputation_change_err(&self) -> Option<ReputationChangeKind> {
        if let Err(err) = self {
            match err {
                RequestError::ChannelClosed |
                RequestError::ConnectionDropped |
                RequestError::UnsupportedCapability |
                RequestError::BadResponse => None,
                RequestError::Timeout => Some(ReputationChangeKind::Timeout),
            }
        } else {
            None
        }
    }
}

/// Error variants that can happen when sending requests to a session.
///
/// Represents errors encountered when sending requests.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RequestError {
    /// Closed channel to the peer.
    #[error("closed channel to the peer")]
    /// Indicates the channel to the peer is closed.
    ChannelClosed,
    /// Connection to a peer dropped while handling the request.
    #[error("connection to a peer dropped while handling the request")]
    /// Represents a dropped connection while handling the request.
    ConnectionDropped,
    /// Capability message is not supported by the remote peer.
    #[error("capability message is not supported by remote peer")]
    /// Indicates an unsupported capability message from the remote peer.
    UnsupportedCapability,
    /// Request timed out while awaiting response.
    #[error("request timed out while awaiting response")]
    /// Represents a timeout while waiting for a response.
    Timeout,
    /// Received bad response.
    #[error("received bad response")]
    /// Indicates a bad response was received.
    BadResponse,
}

// === impl RequestError ===

impl RequestError {
    /// Indicates whether this error is retryable or fatal.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout | Self::ConnectionDropped)
    }

    /// Whether the error happened because the channel was closed.
    pub const fn is_channel_closed(&self) -> bool {
        matches!(self, Self::ChannelClosed)
    }
}

impl<T> From<mpsc::error::SendError<T>> for RequestError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        Self::ChannelClosed
    }
}

impl From<oneshot::error::RecvError> for RequestError {
    fn from(_: oneshot::error::RecvError) -> Self {
        Self::ChannelClosed
    }
}

/// The download result type
pub type DownloadResult<T> = Result<T, DownloadError>;

/// The downloader error type
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /* ==================== HEADER ERRORS ==================== */
    /// Header validation failed.
    #[error("failed to validate header {hash}, block number {number}: {error}")]
    HeaderValidation {
        /// Hash of header failing validation
        hash: B256,
        /// Number of header failing validation
        number: u64,
        /// The details of validation failure
        #[source]
        error: Box<ConsensusError>,
    },
    /// Received an invalid tip.
    #[error("received invalid tip: {0}")]
    InvalidTip(GotExpectedBoxed<B256>),
    /// Received a tip with an invalid tip number.
    #[error("received invalid tip number: {0}")]
    InvalidTipNumber(GotExpected<u64>),
    /// Received a response to a request with unexpected start block
    #[error("headers response starts at unexpected block: {0}")]
    HeadersResponseStartBlockMismatch(GotExpected<u64>),
    /// Received headers with less than expected items.
    #[error("received less headers than expected: {0}")]
    HeadersResponseTooShort(GotExpected<u64>),

    /* ==================== BODIES ERRORS ==================== */
    /// Block validation failed
    #[error("failed to validate body for header {hash}, block number {number}: {error}")]
    BodyValidation {
        /// Hash of the block failing validation
        hash: B256,
        /// Number of the block failing validation
        number: u64,
        /// The details of validation failure
        #[source]
        error: Box<ConsensusError>,
    },
    /// Received more bodies than requested.
    #[error("received more bodies than requested: {0}")]
    TooManyBodies(GotExpected<usize>),
    /// Headers missing from the database.
    #[error("header missing from the database: {block_number}")]
    MissingHeader {
        /// Missing header block number.
        block_number: BlockNumber,
    },
    /// Body range invalid
    #[error("requested body range is invalid: {range:?}")]
    InvalidBodyRange {
        /// Invalid block number range.
        range: RangeInclusive<BlockNumber>,
    },
    /* ==================== COMMON ERRORS ==================== */
    /// Timed out while waiting for request id response.
    #[error("timed out while waiting for response")]
    Timeout,
    /// Received empty response while expecting non empty
    #[error("received empty response")]
    EmptyResponse,
    /// Error while executing the request.
    #[error(transparent)]
    RequestError(#[from] RequestError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

impl From<DatabaseError> for DownloadError {
    fn from(error: DatabaseError) -> Self {
        Self::Provider(ProviderError::Database(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_likely_bad_headers_response() {
        let request =
            HeadersRequest { start: 0u64.into(), limit: 0, direction: Default::default() };
        let headers: Vec<Header> = vec![];
        assert!(!Ok(headers).is_likely_bad_headers_response(&request));

        let request =
            HeadersRequest { start: 0u64.into(), limit: 1, direction: Default::default() };
        let headers: Vec<Header> = vec![];
        assert!(Ok(headers).is_likely_bad_headers_response(&request));
    }
}
