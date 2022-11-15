use super::client::{HeadersClient, HeadersRequest, HeadersStream};
use crate::{consensus::Consensus, p2p::headers::error::DownloadError};
use async_trait::async_trait;
use reth_primitives::{BlockHashOrNumber, Header, SealedHeader};
use reth_rpc_types::engine::ForkchoiceState;
use std::time::Duration;
use tokio_stream::StreamExt;

/// A downloader capable of fetching block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient] represents a client capable of fulfilling these requests.
#[async_trait]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeaderDownloader: Sync + Send {
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
        head: &SealedHeader,
        forkchoice: &ForkchoiceState,
    ) -> Result<Vec<SealedHeader>, DownloadError>;

    /// Perform a header request and returns the headers.
    // TODO: Isn't this effectively blocking per request per downloader?
    // Might be fine, given we can spawn multiple downloaders?
    // TODO: Rethink this function, I don't really like the `stream: &mut HeadersStream`
    // in the signature. Why can we not call `self.client.stream_headers()`? Gives lifetime error.
    async fn download_headers(
        &self,
        stream: &mut HeadersStream,
        start: BlockHashOrNumber,
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
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), DownloadError> {
        if !(parent.hash() == header.parent_hash && parent.number + 1 == header.number) {
            return Err(DownloadError::MismatchedHeaders {
                header_number: header.number.into(),
                parent_number: parent.number.into(),
                header_hash: header.hash(),
                parent_hash: parent.hash(),
            })
        }

        self.consensus()
            .validate_header(header, parent)
            .map_err(|error| DownloadError::HeaderValidation { hash: parent.hash(), error })?;
        Ok(())
    }
}
