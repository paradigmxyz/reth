use super::client::{HeadersClient, HeadersRequest, HeadersStream};
use crate::consensus::Consensus;

use async_trait::async_trait;
use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
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
    /// No headers reponse received
    #[error("Failed to get headers for request {request_id}.")]
    NoHeaderResponse {
        /// The last request ID
        request_id: u64,
    },
    /// Timed out while waiting for request id response.
    #[error("Timed out while getting headers for request {request_id}.")]
    Timeout {
        /// The request id that timed out
        request_id: u64,
    },
}

impl DownloadError {
    /// Returns bool indicating whether this error is retryable or fatal
    pub fn is_retryable(&self) -> bool {
        matches!(self, DownloadError::NoHeaderResponse { .. })
    }
}

/// The header downloading strategy
#[async_trait]
pub trait Downloader: Sync + Send + Debug {
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

    /// Perform a header request. Return the request ID
    async fn download_headers(
        &self,
        stream: &mut HeadersStream,
        start: BlockId,
        limit: u64,
    ) -> Result<Vec<Header>, DownloadError> {
        let request_id = rand::random();
        let request = HeadersRequest { start, limit, reverse: true };
        let _ = self.client().send_header_request(request_id, request).await;

        // let mut stream = self.client().stream_headers();

        // Filter stream by request id and non empty headers content
        let stream = stream.filter_map(|resp| {
            if request_id == resp.id && !resp.headers.is_empty() {
                Some(Ok(resp.headers))
            } else {
                Some(Err(DownloadError::NoHeaderResponse { request_id }))
            }
        });

        let mut stream = Box::pin(stream.timeout(self.timeout()));

        // Pop the first item.
        match stream.next().await {
            Some(item) => item.map_err(|_| DownloadError::Timeout { request_id })?,
            None => return Err(DownloadError::NoHeaderResponse { request_id }),
        }
    }

    /// Validate whether the header is valid in relation to it's parent
    fn validate(
        &self,
        header: &HeaderLocked,
        parent: &HeaderLocked,
    ) -> Result<bool, DownloadError> {
        if !(parent.hash() == header.parent_hash && parent.number + 1 == header.number) {
            return Ok(false)
        }

        self.consensus().validate_header(&header, &parent).map_err(|e| {
            DownloadError::HeaderValidation { hash: parent.hash(), details: e.to_string() }
        })?;
        Ok(true)
    }
}
