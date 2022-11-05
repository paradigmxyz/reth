use super::client::{HeadersClient, HeadersRequest, HeadersStream};
use crate::consensus::Consensus;

use async_trait::async_trait;
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Header, HeaderLocked, H256,
};
use reth_rpc_types::engine::ForkchoiceState;
use std::{fmt::Debug, time::Duration};
use thiserror::Error;
use tokio_stream::StreamExt;

/// The downloader error type
#[derive(Error, Debug, Clone)]
pub enum DownloadError {
    /// Header validation failed
    #[error("Failed to validate header {hash}. Details: {details}.")]
    HeaderValidation {
        /// Hash of header failing validation
        hash: H256,
        /// The details of validation failure
        details: String,
    },
    /// Timed out while waiting for request id response.
    #[error("Timed out while getting headers for request {request_id}.")]
    Timeout {
        /// The request id that timed out
        request_id: u64,
    },
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
}

impl DownloadError {
    /// Returns bool indicating whether this error is retryable or fatal, in the cases
    /// where the peer responds with no headers, or times out.
    pub fn is_retryable(&self) -> bool {
        matches!(self, DownloadError::Timeout { .. })
    }
}

/// The header downloading strategy
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait Downloader: Sync + Send {
    /// The Consensus used to verify block validity when
    /// downloading
    type Consensus: Consensus;

    /// The Client used to download the headers
    type Client: HeadersClient;

    /// The request timeout duration
    fn timeout(&self) -> Duration;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;

    /// The headers client
    fn client(&self) -> &Self::Client;

    /// Download the headers
    async fn download(
        &self,
        head: &HeaderLocked,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError>;

    /// Perform a header request and returns the headers.
    // TODO: Isn't this effectively blocking per request per downloader?
    // Might be fine, given we can spawn multiple downloaders?
    // TODO: Rethink this function, I don't really like the `stream: &mut HeadersStream`
    // in the signature. Why can we not call `self.client.stream_headers()`? Gives lifetime error.
    async fn download_headers(
        &self,
        stream: &mut HeadersStream,
        start: BlockId,
        limit: u64,
    ) -> Result<Vec<Header>, DownloadError> {
        let request_id = rand::random();
        let request = HeadersRequest { start, limit, reverse: true };
        let _ = self.client().send_header_request(request_id, request).await;

        // Filter stream by request id and non empty headers content
        let stream = stream
            .filter(|resp| request_id == resp.id && !resp.headers.is_empty())
            .timeout(self.timeout());

        // Pop the first item.
        match Box::pin(stream).try_next().await {
            Ok(Some(item)) => Ok(item.headers),
            _ => return Err(DownloadError::Timeout { request_id }),
        }
    }

    /// Validate whether the header is valid in relation to it's parent
    ///
    /// Returns Ok(false) if the
    fn validate(&self, header: &HeaderLocked, parent: &HeaderLocked) -> Result<(), DownloadError> {
        if !(parent.hash() == header.parent_hash && parent.number + 1 == header.number) {
            return Err(DownloadError::MismatchedHeaders {
                header_number: header.number.into(),
                parent_number: parent.number.into(),
                header_hash: header.hash(),
                parent_hash: parent.hash(),
            })
        }

        self.consensus().validate_header(header, parent).map_err(|e| {
            DownloadError::HeaderValidation { hash: parent.hash(), details: e.to_string() }
        })?;
        Ok(())
    }
}
