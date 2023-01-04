use futures::{stream::Stream, FutureExt};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        downloader::{DownloadStream, Downloader},
        error::{DownloadError, DownloadResult, PeerRequestResult},
        headers::{
            client::{BlockHeaders, HeadersClient, HeadersRequest},
            downloader::{validate_header_download, HeaderDownloader},
        },
    },
};
use reth_primitives::{HeadersDirection, SealedHeader, H256};
use std::{
    borrow::Borrow,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Download headers in batches
#[derive(Debug)]
pub struct LinearDownloader<C, H> {
    /// The consensus client
    consensus: Arc<C>,
    /// The headers client
    client: Arc<H>,
    /// The batch size per one request
    pub batch_size: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}

impl<C, H> Downloader for LinearDownloader<C, H>
where
    C: Consensus,
    H: HeadersClient,
{
    type Consensus = C;
    type Client = H;

    fn consensus(&self) -> &Self::Consensus {
        self.consensus.borrow()
    }

    fn client(&self) -> &Self::Client {
        self.client.borrow()
    }
}

impl<C, H> HeaderDownloader for LinearDownloader<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    fn stream(&self, head: SealedHeader, tip: H256) -> DownloadStream<'_, SealedHeader> {
        Box::pin(self.new_download(head, tip))
    }
}

impl<C: Consensus, H: HeadersClient> Clone for LinearDownloader<C, H> {
    fn clone(&self) -> Self {
        Self {
            consensus: Arc::clone(&self.consensus),
            client: Arc::clone(&self.client),
            batch_size: self.batch_size,
            request_retries: self.request_retries,
        }
    }
}

impl<C: Consensus, H: HeadersClient> LinearDownloader<C, H> {
    fn new_download(&self, head: SealedHeader, tip: H256) -> HeadersDownload<C, H> {
        HeadersDownload {
            head,
            tip,
            buffered: VecDeque::default(),
            request: Default::default(),
            consensus: Arc::clone(&self.consensus),
            request_retries: self.request_retries,
            batch_size: self.batch_size,
            client: Arc::clone(&self.client),
            encountered_error: false,
        }
    }
}

type HeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<BlockHeaders>> + Send>>;

/// A retryable future that returns a list of [`BlockHeaders`] on success.
struct HeadersRequestFuture {
    request: HeadersRequest,
    fut: HeadersFut,
    retries: usize,
    max_retries: usize,
}

impl HeadersRequestFuture {
    /// Returns true if the request can be retried.
    fn is_retryable(&self) -> bool {
        self.retries < self.max_retries
    }

    /// Increments the retry counter and returns whether the request can still be retried.
    fn inc_err(&mut self) -> bool {
        self.retries += 1;
        self.is_retryable()
    }
}

impl Future for HeadersRequestFuture {
    type Output = PeerRequestResult<BlockHeaders>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().fut.poll_unpin(cx)
    }
}

/// An in progress headers download.
pub struct HeadersDownload<C, H> {
    /// The local head of the chain.
    head: SealedHeader,
    /// Tip to start syncing from
    tip: H256,
    /// Buffered results
    buffered: VecDeque<SealedHeader>,
    /// Contains the request that's currently in progress.
    ///
    /// TODO(mattsse): this could be converted into a `FuturesOrdered` where batching is done via
    /// `skip` so we don't actually need to know the start hash
    request: Option<HeadersRequestFuture>,
    /// Downloader used to issue new requests.
    consensus: Arc<C>,
    /// Downloader used to issue new requests.
    client: Arc<H>,
    /// The number of headers to request in one call
    batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
    /// Flag whether the stream encountered an error
    encountered_error: bool,
}

impl<C, H> HeadersDownload<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    /// Returns the first header from the vector of buffered headers
    fn earliest_header(&self) -> Option<&SealedHeader> {
        self.buffered.back()
    }

    /// Check if any more requests should be dispatched
    fn has_reached_head(&self) -> bool {
        self.earliest_header().map(|h| h.hash() == self.head.hash()).unwrap_or_default()
    }

    /// Check if the stream has terminated
    fn has_terminated(&self) -> bool {
        self.has_reached_head() || self.encountered_error
    }

    /// Returns the start hash for a new request.
    fn request_start(&self) -> H256 {
        self.earliest_header().map_or(self.tip, |h| h.parent_hash)
    }

    /// Get the headers request to dispatch
    fn headers_request(&self) -> HeadersRequest {
        HeadersRequest {
            start: self.request_start().into(),
            limit: self.batch_size,
            direction: HeadersDirection::Rising,
        }
    }

    /// Pop header from the buffer
    fn pop_header_from_buffer(&mut self) -> Option<SealedHeader> {
        if self.buffered.len() > 1 {
            self.buffered.pop_front()
        } else {
            None
        }
    }

    /// Insert the header into buffer
    fn push_header_into_buffer(&mut self, header: SealedHeader) {
        self.buffered.push_back(header);
    }

    /// Get a current future or instantiate a new one
    fn get_or_init_fut(&mut self) -> HeadersRequestFuture {
        match self.request.take() {
            None => {
                // queue in the first request
                let client = Arc::clone(&self.client);
                let req = self.headers_request();
                tracing::trace!(
                    target: "downloaders::headers",
                    "Requesting headers {req:?}"
                );
                HeadersRequestFuture {
                    request: req.clone(),
                    fut: Box::pin(async move { client.get_headers(req).await }),
                    retries: 0,
                    max_retries: self.request_retries,
                }
            }
            Some(fut) => fut,
        }
    }

    /// Tries to fuse the future with a new request.
    ///
    /// Returns an `Err` if the request exhausted all retries
    fn try_fuse_request_fut(&self, fut: &mut HeadersRequestFuture) -> Result<(), ()> {
        if !fut.inc_err() {
            return Err(())
        }
        tracing::trace!(
            target : "downloaders::headers",
            "Retrying future attempt: {}/{}",
            fut.retries,
            fut.max_retries
        );
        let req = self.headers_request();
        fut.request = req.clone();
        let client = Arc::clone(&self.client);
        fut.fut = Box::pin(async move { client.get_headers(req).await });
        Ok(())
    }

    /// Validate whether the header is valid in relation to it's parent
    ///
    /// Returns and `Err` if the header does not conform to consensus rules.
    #[allow(clippy::result_large_err)]
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
        validate_header_download(&self.consensus, header, parent)?;
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn process_header_response(
        &mut self,
        response: PeerRequestResult<BlockHeaders>,
    ) -> DownloadResult<()> {
        match response {
            Ok(res) => {
                let mut headers = res.1 .0;
                headers.sort_unstable_by_key(|h| h.number);

                if headers.is_empty() {
                    return Err(DownloadError::EmptyResponse)
                }

                // Iterate headers in reverse
                for parent in headers.into_iter().rev() {
                    let parent = parent.seal();

                    if self.head.hash() == parent.hash() {
                        // We've reached the target, stop buffering headers
                        self.push_header_into_buffer(parent);
                        break
                    }

                    if let Some(header) = self.earliest_header() {
                        // Proceed to insert. If there is a validation error re-queue
                        // the future.
                        self.validate(header, &parent)?;
                    } else if parent.hash() != self.tip {
                        // The buffer is empty and the first header does not match the
                        // tip, requeue the future
                        return Err(DownloadError::InvalidTip {
                            received: parent.hash(),
                            expected: self.tip,
                        })
                    }

                    // Record new parent
                    self.push_header_into_buffer(parent);
                }
                Ok(())
            }
            // most likely a noop, because this error
            // would've been handled by the fetcher internally
            Err(err) => Err(err.into()),
        }
    }
}

impl<C, H> Stream for HeadersDownload<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    type Item = DownloadResult<SealedHeader>;

    /// Linear header downloader implemented as a [Stream]. The downloader sends header
    /// requests until the head is reached and buffers the responses. If the request future
    /// is still pending, the downloader will return a buffered header if any is available.
    ///
    /// Internally, the stream is terminated if the `done` flag has been set and there are no
    /// more headers available in the buffer.
    ///
    /// Upon encountering an error, the downloader will attempt to retry the failed request.
    /// If the number of retries is exhausted, the downloader will stream an error,
    /// clear the buffered headers, thus resulting in stream termination.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Drain any buffered element except the head
            if let Some(header) = this.pop_header_from_buffer() {
                return Poll::Ready(Some(Ok(header)))
            }

            // We've reached the head or encountered an error, terminate the stream
            if this.has_terminated() {
                return Poll::Ready(None)
            }

            // Initialize the future
            let mut fut = this.get_or_init_fut();
            match fut.poll_unpin(cx) {
                Poll::Ready(result) => {
                    let peer_id = result.as_ref().map(|res| res.peer_id()).ok();
                    // Process the response, buffering the headers
                    // in case of successful validation
                    match this.process_header_response(result) {
                        Ok(_) => {
                            // The buffer has been updated, check if we need
                            // to dispatch another request
                            if !this.has_reached_head() {
                                // Create a new request
                                this.request = Some(this.get_or_init_fut());
                            }
                        }
                        Err(error) => {
                            tracing::error!(
                                target: "downloaders::headers", request = ?fut.request, ?error, "Error processing header response",
                            );
                            // Penalize the peer for bad response
                            if let Some(peer_id) = peer_id {
                                tracing::trace!(target: "downloaders::headers", ?peer_id, ?error, "Penalizing peer");
                                this.client.report_bad_message(peer_id);
                            }
                            // Response is invalid, attempt to retry
                            if this.try_fuse_request_fut(&mut fut).is_err() {
                                tracing::trace!(
                                    target: "downloaders::headers",
                                    "Ran out of retries, terminating stream"
                                );
                                // We exhausted all of the retries. Stream must terminate
                                this.buffered.clear();
                                this.encountered_error = true;
                                return Poll::Ready(Some(Err(error)))
                            }
                            // Reset the future
                            this.request = Some(fut);
                        }
                    };
                }
                Poll::Pending => {
                    // Set the future back to state
                    this.request = Some(fut);
                    return Poll::Pending
                }
            }
        }
    }
}

/// The builder for [LinearDownloader] with
/// some default settings
#[derive(Debug)]
pub struct LinearDownloadBuilder {
    /// The batch size per one request
    batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
}

impl Default for LinearDownloadBuilder {
    fn default() -> Self {
        Self { batch_size: 100, request_retries: 5 }
    }
}

impl LinearDownloadBuilder {
    /// Set the request batch size
    pub fn batch_size(mut self, size: u64) -> Self {
        self.batch_size = size;
        self
    }

    /// Set the number of retries per request
    pub fn retries(mut self, retries: usize) -> Self {
        self.request_retries = retries;
        self
    }

    /// Build [LinearDownloader] with provided consensus
    /// and header client implementations
    pub fn build<C: Consensus, H: HeadersClient>(
        self,
        consensus: Arc<C>,
        client: Arc<H>,
    ) -> LinearDownloader<C, H> {
        LinearDownloader {
            consensus,
            client,
            batch_size: self.batch_size,
            request_retries: self.request_retries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;
    use once_cell::sync::Lazy;
    use reth_interfaces::test_utils::{TestConsensus, TestHeadersClient};
    use reth_primitives::SealedHeader;

    static CONSENSUS: Lazy<Arc<TestConsensus>> = Lazy::new(|| Arc::new(TestConsensus::default()));

    fn child_header(parent: &SealedHeader) -> SealedHeader {
        let mut child = parent.as_ref().clone();
        child.number += 1;
        child.parent_hash = parent.hash_slow();
        let hash = child.hash_slow();
        SealedHeader::new(child, hash)
    }

    #[tokio::test]
    async fn stream_empty() {
        let client = Arc::new(TestHeadersClient::default());
        let downloader =
            LinearDownloadBuilder::default().build(CONSENSUS.clone(), Arc::clone(&client));

        let result = downloader
            .stream(SealedHeader::default(), H256::default())
            .try_collect::<Vec<_>>()
            .await;
        assert!(result.is_err());
        assert_eq!(client.request_attempts(), downloader.request_retries as u64);
    }

    #[tokio::test]
    async fn download_at_fork_head() {
        let client = Arc::new(TestHeadersClient::default());
        let downloader = LinearDownloadBuilder::default()
            .batch_size(3)
            .build(CONSENSUS.clone(), Arc::clone(&client));

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let result = downloader.stream(p0.clone(), p0.hash_slow()).try_collect::<Vec<_>>().await;
        let headers = result.unwrap();
        assert!(headers.is_empty());
        assert_eq!(client.request_attempts(), 1);
    }

    #[tokio::test]
    async fn download_exact() {
        let client = Arc::new(TestHeadersClient::default());
        let downloader = LinearDownloadBuilder::default()
            .batch_size(3)
            .build(CONSENSUS.clone(), Arc::clone(&client));

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let result = downloader.stream(p3, p0.hash_slow()).try_collect::<Vec<_>>().await;
        let headers = result.unwrap();
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0], p0);
        assert_eq!(headers[1], p1);
        assert_eq!(headers[2], p2);
        // stream has to poll twice because of the batch size
        assert_eq!(client.request_attempts(), 2);
    }
}
