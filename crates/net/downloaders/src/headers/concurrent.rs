//! A headers downloader that can handle multiple requests concurrently.

use futures::{stream::Stream, FutureExt};
use futures_util::{stream::FuturesUnordered, StreamExt};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        downloader::Downloader,
        error::{DownloadError, DownloadResult, PeerRequestResult},
        headers::{
            client::{BlockHeaders, HeadersClient, HeadersRequest},
            downloader::HeaderDownloader2,
        },
    },
};
use reth_primitives::{Header, HeadersDirection, PeerId, SealedHeader, H256};
use std::{
    borrow::Borrow,
    cmp::{Ordering, Reverse},
    collections::{binary_heap::PeekMut, BinaryHeap},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

/// Download headers concurrently
#[must_use = "Stream does nothing unless polled"]
pub struct ConcurrentDownloader<C, H> {
    /// Consensus client used to validate headers
    consensus: Arc<C>,
    /// Client used to download headers.
    client: Arc<H>,
    /// The local head of the chain.
    local_head: SealedHeader,
    /// The block number to use for requests.
    next_request_block_number: u64,
    /// The header to validate against.
    earliest_header: Option<SealedHeader>,
    /// Tip block number to start validating from (in reverse)
    next_chain_tip_block_number: u64,
    /// Tip hash start syncing from (in reverse)
    next_chain_tip_hash: H256,
    /// The batch size per one request
    request_batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
    /// Maximum amount of requests to handle concurrently.
    max_concurrent_requests: usize,
    /// The number of block headers to return at once
    stream_batch_size: usize,
    /// Maximum amount of received headers to buffer internally.
    max_buffer_size: usize,
    /// requests in progress
    in_progress_queue: FuturesUnordered<HeadersRequestFuture>,
    /// Buffered, unvalidated responses
    buffered_responses: BinaryHeap<OrderedHeadersResponse>,
    /// Buffered, validated headers ready to be returned
    queued_validated_headers: Vec<SealedHeader>,
    /// Whether this stream is terminated
    is_terminated: bool,
}

// === impl ConcurrentDownloader ===

impl<C, H> ConcurrentDownloader<C, H>
where
    C: Consensus + 'static,
    H: HeadersClient + 'static,
{
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
        if self.next_request_block_number > local_head {
            // downloading is in reverse
            let diff = self.next_request_block_number - local_head;
            let limit = diff.min(self.request_batch_size);
            let start = self.next_request_block_number;
            self.next_request_block_number -= limit;

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
        self.queued_validated_headers.last().or(self.earliest_header.as_ref())
    }

    /// Processes the next headers in line.
    ///
    /// This will validate all headers and insert them into the validated buffer.
    ///
    /// Returns an error if the given headers are invalid
    #[allow(clippy::result_large_err)]
    fn process_next_headers(
        &mut self,
        request: HeadersRequest,
        headers: Vec<Header>,
        peer_id: PeerId,
        retries: usize,
    ) -> Result<(), HeadersResponseError> {
        for parent in headers {
            let parent = parent.seal();

            // Validate the headers
            if let Some(header) = self.earliest_header() {
                if let Err(error) = self.validate(header, &parent) {
                    return Err(HeadersResponseError {
                        request,
                        retries,
                        peer_id: Some(peer_id),
                        error,
                    })
                }
            } else if parent.hash() != self.next_chain_tip_hash {
                return Err(HeadersResponseError {
                    request,
                    retries,
                    peer_id: Some(peer_id),
                    error: DownloadError::InvalidTip {
                        received: parent.hash(),
                        expected: self.next_chain_tip_hash,
                    },
                })
            }

            // update tracked block info
            self.next_chain_tip_hash = parent.hash();
            self.next_chain_tip_block_number = parent.number;
            self.queued_validated_headers.push(parent);
        }

        Ok(())
    }

    /// Invoked when we received a response
    #[allow(clippy::result_large_err)]
    fn on_headers_outcome(
        &mut self,
        response: HeadersRequestOutcome,
    ) -> Result<(), HeadersResponseError> {
        let requested_block_number = response.block_number();
        let HeadersRequestOutcome { request, retries, outcome } = response;

        match outcome {
            Ok(res) => {
                let (peer_id, headers) = res.split();
                let mut headers = headers.0;

                // sort headers from highest to lowest block number
                headers.sort_unstable_by_key(|h| Reverse(h.number));

                if headers.is_empty() {
                    return Err(HeadersResponseError {
                        request,
                        retries,
                        peer_id: Some(peer_id),
                        error: DownloadError::EmptyResponse,
                    })
                }

                // validate the response
                let highest = &headers[0];

                if highest.number != requested_block_number {
                    // TODO return error, invalid response
                }

                if (headers.len() as u64) != request.limit {
                    // TODO decide how to treat this
                }

                // check if the response is the next expected
                if highest.number == self.next_chain_tip_block_number {
                    // is next response, validate it
                    self.process_next_headers(request, headers, peer_id, retries)?;

                    // try to validate all buffered responses blocked by this successful response
                    self.try_validate_buffered()
                        .map(Err::<(), HeadersResponseError>)
                        .transpose()?;
                } else {
                    // can't validate yet
                    self.buffered_responses.push(OrderedHeadersResponse {
                        headers,
                        request,
                        retries,
                        peer_id,
                    })
                }

                Ok(())
            }
            // most likely a noop, because this error
            // would've been handled by the fetcher internally
            Err(err) => {
                Err(HeadersResponseError { request, retries, peer_id: None, error: err.into() })
            }
        }
    }

    /// Handles the error of a bad response
    ///
    /// Returns the error if the request ran out if retries.
    fn on_headers_error(&mut self, err: HeadersResponseError) -> Option<DownloadError> {
        let HeadersResponseError { request, retries, peer_id, error } = err;

        // Penalize the peer for bad response
        if let Some(peer_id) = peer_id {
            tracing::trace!(target: "downloaders::headers", ?peer_id, ?error, "Penalizing peer");
            self.client.report_bad_message(peer_id);
        }

        if retries < self.request_retries {
            // re-submit the request
            self.submit_request(request, retries + 1);
            None
        } else {
            tracing::trace!(
                target: "downloaders::headers",
                "Ran out of retries"
            );
            Some(error)
        }
    }

    /// Attempts to validate the buffered responses
    ///
    /// Returns an error if the next expected response was popped, but failed validation
    fn try_validate_buffered(&mut self) -> Option<HeadersResponseError> {
        loop {
            // Check to see if we've already received the next value
            let next_response = self.buffered_responses.peek_mut()?;
            if next_response.block_number() == self.next_request_block_number {
                let OrderedHeadersResponse { headers, request, retries, peer_id } =
                    PeekMut::pop(next_response);
                if let Err(err) = self.process_next_headers(request, headers, peer_id, retries) {
                    return Some(err)
                }
            } else {
                return None
            }
        }
    }

    /// Starts a request future
    fn submit_request(&mut self, request: HeadersRequest, retries: usize) {
        let client = Arc::clone(&self.client);
        let fut = HeadersRequestFuture {
            request: Some(request.clone()),
            fut: Box::pin(async move { client.get_headers(request).await }),
            retries,
        };
        self.in_progress_queue.push(fut);
    }

    /// Clears all requests/responses.
    fn clear(&mut self) {
        self.queued_validated_headers.clear();
        self.buffered_responses.clear();
        self.in_progress_queue.clear();
    }

    fn terminate(&mut self) {
        self.is_terminated = true;
        self.clear();
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

        if this.is_terminated {
            return Poll::Ready(None)
        }

        // yield next batch
        if this.queued_validated_headers.len() > this.stream_batch_size {
            let next_batch = this.queued_validated_headers.split_off(this.stream_batch_size);
            return Poll::Ready(Some(Ok(next_batch)))
        }

        // this will submit new requests and poll them
        loop {
            // populate requests
            while this.in_progress_queue.len() < this.max_concurrent_requests() &&
                this.buffered_responses.len() < this.max_buffer_size
            {
                if let Some(request) = this.next_request() {
                    tracing::trace!(
                        target: "downloaders::headers",
                        "Requesting headers {request:?}"
                    );
                    this.submit_request(request, 0);
                } else {
                    // no more requests
                    break
                }
            }

            // poll requests
            if let Poll::Ready(Some(outcome)) = this.in_progress_queue.poll_next_unpin(cx) {
                // handle response
                if let Err(err) = this.on_headers_outcome(outcome) {
                    if let Some(fatal) = this.on_headers_error(err) {
                        // encountered fatal error: terminate the stream
                        this.terminate();
                        return Poll::Ready(Some(Err(fatal)))
                    }
                }
            } else {
                break
            }
        }

        // all requests are handled, stream is finished
        if this.in_progress_queue.is_empty() {
            if this.queued_validated_headers.is_empty() {
                return Poll::Ready(None)
            }

            // return the last validated headers
            return Poll::Ready(Some(Ok(std::mem::take(&mut this.queued_validated_headers))))
        }

        Poll::Pending
    }
}

/// SAFETY: we need to ensure `ConcurrentDownloader` is `Sync` because the of the [Downloader]
/// trait. While [HeadersClient] is also `Sync`, the [HeadersClient::get_headers] future does not
/// enforce `Sync` (async_trait). The future itself does not use any interior mutability whatsoever:
/// All the mutations are performed through an exclusive reference on `ConcurrentDownloader` when
/// the Stream is polled. This means it suffices that `ConcurrentDownloader` is Sync:
unsafe impl<C, H> Sync for ConcurrentDownloader<C, H>
where
    C: Consensus,
    H: HeadersClient,
{
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
    peer_id: PeerId,
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

/// Type returned if a bad response was processed
struct HeadersResponseError {
    request: HeadersRequest,
    retries: usize,
    peer_id: Option<PeerId>,
    error: DownloadError,
}

/// The builder for [ConcurrentDownloader] with
/// some default settings
#[derive(Debug)]
pub struct ConcurrentDownloadBuilder {
    /// The batch size per one request
    batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
}

impl Default for ConcurrentDownloadBuilder {
    fn default() -> Self {
        Self { batch_size: 1_000, request_retries: 5 }
    }
}

impl ConcurrentDownloadBuilder {
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
        _consensus: Arc<C>,
        _client: Arc<H>,
    ) -> ConcurrentDownloader<C, H> {
        todo!()
    }
}

#[cfg(test)]
mod tests {

    // static CONSENSUS: Lazy<Arc<TestConsensus>> = Lazy::new(||
    // Arc::new(TestConsensus::default()));
    //
    // fn child_header(parent: &SealedHeader) -> SealedHeader {
    //     let mut child = parent.as_ref().clone();
    //     child.number += 1;
    //     child.parent_hash = parent.hash_slow();
    //     let hash = child.hash_slow();
    //     SealedHeader::new(child, hash)
    // }
    //
    // #[tokio::test]
    // async fn stream_empty() {
    //     let client = Arc::new(TestHeadersClient::default());
    //     let downloader =
    //         ConcurrentDownloadBuilder::default().build(CONSENSUS.clone(), Arc::clone(&client));
    //
    //     let result = downloader
    //         .stream(SealedHeader::default(), H256::default())
    //         .try_collect::<Vec<_>>()
    //         .await;
    //     assert!(result.is_err());
    //     assert_eq!(client.request_attempts(), downloader.request_retries as u64);
    // }
    //
    // #[tokio::test]
    // async fn download_at_fork_head() {
    //     let client = Arc::new(TestHeadersClient::default());
    //     let downloader = ConcurrentDownloadBuilder::default()
    //         .batch_size(3)
    //         .build(CONSENSUS.clone(), Arc::clone(&client));
    //
    //     let p3 = SealedHeader::default();
    //     let p2 = child_header(&p3);
    //     let p1 = child_header(&p2);
    //     let p0 = child_header(&p1);
    //
    //     client
    //         .extend(vec![
    //             p0.as_ref().clone(),
    //             p1.as_ref().clone(),
    //             p2.as_ref().clone(),
    //             p3.as_ref().clone(),
    //         ])
    //         .await;
    //
    //     let result = downloader.stream(p0.clone(), p0.hash_slow()).try_collect::<Vec<_>>().await;
    //     let headers = result.unwrap();
    //     assert!(headers.is_empty());
    //     assert_eq!(client.request_attempts(), 1);
    // }
    //
    // #[tokio::test]
    // async fn download_exact() {
    //     let client = Arc::new(TestHeadersClient::default());
    //     let downloader = ConcurrentDownloadBuilder::default()
    //         .batch_size(3)
    //         .build(CONSENSUS.clone(), Arc::clone(&client));
    //
    //     let p3 = SealedHeader::default();
    //     let p2 = child_header(&p3);
    //     let p1 = child_header(&p2);
    //     let p0 = child_header(&p1);
    //
    //     client
    //         .extend(vec![
    //             p0.as_ref().clone(),
    //             p1.as_ref().clone(),
    //             p2.as_ref().clone(),
    //             p3.as_ref().clone(),
    //         ])
    //         .await;
    //
    //     let result = downloader.stream(p3, p0.hash_slow()).try_collect::<Vec<_>>().await;
    //     let headers = result.unwrap();
    //     assert_eq!(headers.len(), 3);
    //     assert_eq!(headers[0], p0);
    //     assert_eq!(headers[1], p1);
    //     assert_eq!(headers[2], p2);
    //     // stream has to poll twice because of the batch size
    //     assert_eq!(client.request_attempts(), 2);
    // }
}
