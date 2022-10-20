use async_trait::async_trait;
use rand::Rng;
use reth_interfaces::{
    consensus::Consensus,
    stages::{HeaderRequest, HeadersClient, MessageStream},
};
use reth_primitives::{rpc::BlockId, Header, HeaderLocked, H256};
use reth_rpc_types::engine::ForkchoiceState;
use std::{fmt::Debug, sync::Arc, time::Duration};
use thiserror::Error;
use tokio_stream::StreamExt;

/// The header downloading strategy
#[async_trait]
pub trait Downloader: Sync + Send + Debug {
    /// The Consensus used to verify block validity when
    /// downloading
    type Consensus: Consensus;

    /// The request timeout in seconds
    fn timeout(&self) -> u64;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;

    /// Download the headers
    async fn download(
        &self,
        client: Arc<dyn HeadersClient>,
        head: &HeaderLocked,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<HeaderLocked>, DownloadError>;

    /// Perform a header request. Return the request ID
    async fn download_headers(
        &self,
        stream: &mut MessageStream<(u64, Vec<Header>)>,
        client: Arc<dyn HeadersClient>,
        start: BlockId,
        limit: u64,
    ) -> Result<Vec<Header>, DownloadError> {
        let request_id = rand::thread_rng().gen();
        let request = HeaderRequest { start, limit, reverse: true };
        let _ = client.send_header_request(request_id, request).await;

        // Filter stream by request id and non empty headers content
        let stream = stream.filter(|(id, headers)| request_id == *id && !headers.is_empty());

        // Wrap the stream with a timeout
        let stream = stream.timeout(Duration::from_secs(self.timeout()));
        match Box::pin(stream).try_next().await {
            Ok(Some((_, h))) => Ok(h),
            _ => return Err(DownloadError::NoHeaderResponse { request_id }),
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
}

impl DownloadError {
    /// Returns bool indicating whether this error is retryable or fatal
    pub fn is_retryable(&self) -> bool {
        matches!(self, DownloadError::NoHeaderResponse { .. })
    }
}
