use crate::{
    consensus::{self, Consensus},
    p2p::error::RequestError,
};
use futures::Stream;
use reth_primitives::{rpc::BlockNumber, PeerId, H256};
use std::{fmt::Debug, pin::Pin};
use thiserror::Error;

/// A stream for downloading response.
pub type DownloadStream<'a, T> = Pin<Box<dyn Stream<Item = DownloadResult<T>> + Send + 'a>>;

/// Generic download client for peer penalization
pub trait DownloadClient: Send + Sync + Debug {
    /// Penalize the peer for responding with a message
    /// that violates validation rules
    fn penalize(&self, peer_id: PeerId);
}

/// The generic trait for requesting and verifying data
/// over p2p network client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait Downloader: Send + Sync {
    /// The client used to fetch necessary data
    type Client: DownloadClient;

    /// The Consensus used to verify data validity when downloading
    type Consensus: Consensus;

    /// The headers client
    fn client(&self) -> &Self::Client;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;
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
    /// Error while executing the request.
    #[error(transparent)]
    RequestError(#[from] RequestError),
}
