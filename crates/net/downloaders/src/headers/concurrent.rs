use futures::{stream::Stream, FutureExt};
use futures_util::{stream::FuturesUnordered, StreamExt};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        downloader::{DownloadStream, Downloader},
        error::{DownloadError, DownloadResult, PeerRequestResult},
        headers::{
            client::{BlockHeaders, HeadersClient, HeadersRequest},
            downloader::{validate_header_download, HeaderDownloader, HeaderDownloader2},
        },
    },
};
use reth_primitives::{Header, HeadersDirection, SealedHeader, H256};
use std::{
    borrow::Borrow,
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

/// Download headers concurrently
#[derive(Debug)]
pub struct ConcurrentDownloader<C, H> {
    /// Consensus client used to validate headers
    consensus: Arc<C>,
    /// Client used to download headers.
    client: Arc<H>,
    /// The local head of the chain.
    local_head: SealedHeader,
    /// The block number to use for requests.
    next_request_number: u64,
    /// The header to validate against.
    earliest_header: Option<SealedHeader>,
    /// Tip block number to start validating from (in reverse)
    next_chain_tip_number: u64,
    /// Tip hash start syncing from (in reverse)
    next_chain_tip_hash: H256,
    /// The batch size per one request
    request_batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
    /// Maximum amount of requests to handle concurrently.
    max_concurrent_requests: usize,
    /// Maximum amount of received headers to buffer internally.
    max_buffer_size: usize,
    /// requests in progress
    in_progress_queue: FuturesUnordered<HeadersRequestFuture>,
    /// Buffered, unvalidated responses
    buffered_responses: BinaryHeap<OrderedHeadersResponse>,
    /// Buffered, validated headers ready to be returned
    queued_validated_headers: Vec<SealedHeader>,
}

// === impl ConcurrentDownloader ===

impl<C: Consensus, H: HeadersClient> ConcurrentDownloader<C, H> {
    /// Returns the block number the local node is at.
    fn local_block_number(&self) -> u64 {
        self.local_head.number
    }

    /// Max requests to handle at the same time
    fn max_concurrent_requests(&self) -> usize {
        self.max_concurrent_requests
    }

    /// Returns the next header request
    ///
    /// This will advance the current block towards the local head.
    ///
    /// Returns `None` if no more requests are required.
    fn next_request(&mut self) -> Option<HeadersRequest> {
        let local_head = self.local_block_number();
        if self.next_request_number > local_head {
            // downloading is in reverse
            let diff = self.next_request_number - local_head;
            let limit = diff.min(self.request_batch_size);
            let start = self.next_request_number;
            self.next_request_number -= limit;

            return Some(HeadersRequest {
                start: start.into(),
                limit,
                direction: HeadersDirection::Falling,
            })
        }

        None
    }

    /// Returns the first header to use for validation.
    fn earliest_header(&self) -> Option<&SealedHeader> {
        self.queued_validated_headers.last().or_else(|| self.earliest_header.as_ref())
    }

    /// Invoked when we received a response
    #[allow(clippy::result_large_err)]
    fn on_headers_response(&mut self, response: HeadersRequestOutcome) -> DownloadResult<()> {
        let requested_block_number = response.block_number();
        let HeadersRequestOutcome { request, retries, outcome } = response;

        match outcome {
            Ok(res) => {
                let mut headers = res.1 .0;
                // sort headers from highest to lowest block number
                headers.sort_unstable_by_key(|h| Reverse(h.number));

                if headers.is_empty() {
                    return Err(DownloadError::EmptyResponse)
                }

                // validate the response
                let highest = &headers[0];

                if highest.number != requested_block_number {
                    // TODO return error, invalid response
                }

                if headers.len() != request.limit {
                    // TODO decide how to treat this
                }

                if highest.number == self.next_chain_tip_number {
                    // is next response, validate it
                    for parent in headers {
                        let parent = parent.seal();

                        if let Some(header) = self.earliest_header() {
                            // Proceed to insert. If there is a validation error re-queue
                            // the future.
                            self.validate(header, &parent).unwrap();
                        } else if parent.hash() != self.next_chain_tip_hash {
                            // TODO invalid request

                            // // The buffer is empty and the first header does not match the
                            // // tip, requeue the future
                            // return Err(DownloadError::InvalidTip {
                            //     received: parent.hash(),
                            //     expected: self.tip,
                            // })
                        }

                        // update tracked block info
                        self.next_chain_tip_hash = parent.hash();
                        self.next_chain_tip_number = parent.number;
                        self.queued_validated_headers.push(parent);
                    }

                    // TODO also check for buffered response that can be validated now
                } else {
                    // can't validate yet
                    self.buffered_responses.push(OrderedHeadersResponse {
                        headers,
                        request,
                        retries,
                    })
                }

                Ok(())
            }
            // most likely a noop, because this error
            // would've been handled by the fetcher internally
            Err(err) => Err(err.into()),
        }
    }
}

impl<C, H> Downloader for ConcurrentDownloader<C, H>
where
    C: Consensus,
    H: HeadersClient,
{
    type Client = H;
    type Consensus = C;

    fn client(&self) -> &Self::Client {
        self.client.borrow()
    }

    fn consensus(&self) -> &Self::Consensus {
        self.consensus.borrow()
    }
}

impl<C, H> HeaderDownloader2 for ConcurrentDownloader<C, H>
where
    C: Consensus,
    H: HeadersClient,
{
}

impl<C, H> Stream for ConcurrentDownloader<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
    type Item = DownloadResult<Vec<SealedHeader>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // TODO pop responses

        loop {
            while this.in_progress_queue.len() < this.max_concurrent_requests() {
                if let Some(request) = this.next_request() {
                    tracing::trace!(
                        target: "downloaders::headers",
                        "Requesting headers {request:?}"
                    );
                    let client = Arc::clone(&this.client);
                    let fut = HeadersRequestFuture {
                        request: Some(request.clone()),
                        fut: Box::pin(async move { client.get_headers(request).await }),
                        retries: 0,
                    };
                    this.in_progress_queue.push(fut);
                } else {
                    // no more requests
                    break
                }
            }

            // poll requests
            if let Poll::Ready(Some(outcome)) = this.in_progress_queue.poll_next_unpin(cx) {
                // handle response
                match this.on_headers_response(outcome) {
                    Ok(_) => {}
                    Err(_) => {}
                }
            } else {
                break
            }
        }

        if this.in_progress_queue.is_empty() {
            return Poll::Ready(None)
        }

        Poll::Pending
    }
}

type HeadersFut = Pin<Box<dyn Future<Output = PeerRequestResult<BlockHeaders>> + Send>>;

/// A future that returns a list of [`BlockHeaders`] on success.
struct HeadersRequestFuture {
    request: Option<HeadersRequest>,
    fut: HeadersFut,
    retries: usize,
}

impl Future for HeadersRequestFuture {
    type Output = HeadersRequestOutcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let outcome = ready!(this.fut.poll_unpin(cx));
        let request = this.request.take().unwrap();
        Poll::Ready(HeadersRequestOutcome { request, retries: this.retries, outcome })
    }
}

/// The outcome of the [HeadersRequestFuture]
struct HeadersRequestOutcome {
    request: HeadersRequest,
    retries: usize,
    outcome: PeerRequestResult<BlockHeaders>,
}

// === impl OrderedHeadersResponse ===

impl HeadersRequestOutcome {
    fn block_number(&self) -> u64 {
        self.request.start.as_number().expect("is number")
    }
}

/// Wrapper type to order responses
struct OrderedHeadersResponse {
    headers: Vec<Header>,
    request: HeadersRequest,
    retries: usize,
}

// === impl OrderedHeadersResponse ===

impl OrderedHeadersResponse {
    fn block_number(&self) -> u64 {
        self.request.start.as_number().expect("is number")
    }
}

impl PartialEq for OrderedHeadersResponse {
    fn eq(&self, other: &Self) -> bool {
        self.block_number() == other.block_number()
    }
}

impl Eq for OrderedHeadersResponse {}

impl PartialOrd for OrderedHeadersResponse {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedHeadersResponse {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max heap, so compare backwards here.
        other.block_number().cmp(&self.block_number())
    }
}

/// The builder for [ConcurrentDownloader] with
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
        Self { batch_size: 1_000, request_retries: 5 }
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

    /// Build [ConcurrentDownloader] with provided consensus
    /// and header client implementations
    pub fn build<C: Consensus, H: HeadersClient>(
        self,
        consensus: Arc<C>,
        client: Arc<H>,
    ) -> ConcurrentDownloader<C, H> {
        todo!()
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
