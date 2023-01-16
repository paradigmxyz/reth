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

/// Download headers concurrently.
///
/// This [Downloader] downloads headers using the configured [HeadersClient].
/// Headers can be requested by hash or block number and take a `limit` parameter. This downloader
/// tries to fill the gap between the local head of the node and the chain tip by issuing multiple
/// requests at a time but yielding them in batches on [Stream::poll_next].
///
/// **Note:** This downloader downloads in reverse, see also [HeadersDirection::Falling], this means
/// the batches of headers that this downloader yields will start at the chain tip and move towards
/// the local head: falling block numbers.
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
    /// A buffered header to validate against.
    ///
    /// This is only used to guarantee that we always have the lowest validated header, even if
    /// `Stream::poll_next` drained all `queued_validated_headers`.
    lowest_validated_header: Option<SealedHeader>,
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
    max_buffered_responses: usize,
    /// requests in progress
    in_progress_queue: FuturesUnordered<HeadersRequestFuture>,
    /// Buffered, unvalidated responses
    buffered_responses: BinaryHeap<OrderedHeadersResponse>,
    /// Buffered, _sorted_ and validated headers ready to be returned.
    ///
    /// Note: headers are sorted from high to low
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
    #[inline]
    fn local_block_number(&self) -> u64 {
        self.local_head.number
    }

    /// Max requests to handle at the same time
    #[inline]
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
            let request = calc_next_request(
                local_head,
                self.next_request_block_number,
                self.request_batch_size,
            );

            // need to shift the tracked request block number based on the number of requested
            // headers so follow-up requests will use that as start.
            self.next_request_block_number -= request.limit;

            return Some(request)
        }

        None
    }

    /// Returns the next header to use for validation.
    ///
    /// Since this downloader downloads blocks with falling block number, this will return the
    /// lowest (in terms of block number) validated header.
    ///
    /// This is either the last `queued_validated_headers`, or if has been drained entirely the
    /// `lowest_validated_header`.
    ///
    /// This only returns `None` if we haven't fetched the initial chain tip yet.
    fn lowest_validated_header(&self) -> Option<&SealedHeader> {
        self.queued_validated_headers.last().or(self.lowest_validated_header.as_ref())
    }

    /// Processes the next headers in line.
    ///
    /// This will validate all headers and insert them into the validated buffer.
    ///
    /// Returns an error if the given headers are invalid.
    ///
    /// Caution: this expects the `headers` to be sorted.
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
            if let Some(validated_header) = self.lowest_validated_header() {
                if let Err(error) = self.validate(validated_header, &parent) {
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
            self.next_chain_tip_hash = parent.parent_hash;
            self.next_chain_tip_block_number = parent.number - 1;
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
                    return Err(HeadersResponseError {
                        request,
                        retries,
                        peer_id: Some(peer_id),
                        error: DownloadError::HeadersResponseStartBlockMismatch {
                            received: highest.number,
                            expected: requested_block_number,
                        },
                    })
                }

                if (headers.len() as u64) != request.limit {
                    return Err(HeadersResponseError {
                        retries,
                        peer_id: Some(peer_id),
                        error: DownloadError::HeadersResponseTooShort {
                            received: headers.len() as u64,
                            expected: request.limit,
                        },
                        request,
                    })
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
    /// Returns an error if the next expected response was popped, but failed validation.
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
        self.lowest_validated_header.take();
        self.queued_validated_headers.clear();
        self.buffered_responses.clear();
        self.in_progress_queue.clear();
    }

    /// Marks the stream as terminated
    fn terminate(&mut self) {
        self.is_terminated = true;
        self.clear();
    }

    /// Splits off the next batch
    ///
    /// # Panics
    ///
    /// if `len(queued_validated_headers) < stream_batch_size`
    fn split_next_batch(&mut self) -> Vec<SealedHeader> {
        let mut rem = self.queued_validated_headers.split_off(self.stream_batch_size);
        std::mem::swap(&mut rem, &mut self.queued_validated_headers);
        rem
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
    C: Consensus + 'static,
    H: HeadersClient + 'static,
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
            let next_batch = this.split_next_batch();

            // Note: if this would drain all headers, we need to keep the lowest (last index) around
            // so we can continue validating headers responses.
            if this.queued_validated_headers.is_empty() {
                this.lowest_validated_header = next_batch.last().cloned();
            }

            return Poll::Ready(Some(Ok(next_batch)))
        }

        // this will submit new requests and poll them
        loop {
            // populate requests
            while this.in_progress_queue.len() < this.max_concurrent_requests() &&
                this.buffered_responses.len() < this.max_buffered_responses
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
                this.terminate();
                return Poll::Ready(None)
            }

            if this.queued_validated_headers.len() > this.stream_batch_size {
                let next_batch = this.split_next_batch();
                return Poll::Ready(Some(Ok(next_batch)))
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
        self.block_number().cmp(&other.block_number())
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
    request_batch_size: u64,
    /// The number of retries for downloading
    request_retries: usize,
    /// Batch size for headers
    stream_batch_size: usize,
    /// Batch size for headers
    max_concurrent_requests: usize,
    /// How many responses to buffer
    max_buffered_responses: usize,
}

impl Default for ConcurrentDownloadBuilder {
    fn default() -> Self {
        Self {
            request_batch_size: 1_000,
            request_retries: 5,
            stream_batch_size: 10_000,
            max_concurrent_requests: 100,
            max_buffered_responses: 500,
        }
    }
}

impl ConcurrentDownloadBuilder {
    /// Set the request batch size
    pub fn request_batch_size(mut self, size: u64) -> Self {
        self.request_batch_size = size;
        self
    }

    /// Set the request batch size
    pub fn stream_batch_size(mut self, size: usize) -> Self {
        self.stream_batch_size = size;
        self
    }

    /// Set the number of retries per request
    pub fn retries(mut self, retries: usize) -> Self {
        self.request_retries = retries;
        self
    }

    /// Set the max amount of concurrent requests.
    pub fn max_concurrent_requests(mut self, retries: usize) -> Self {
        self.request_retries = retries;
        self
    }

    /// How many responses to buffer internally
    pub fn max_buffered_responses(mut self, max_buffered_responses: usize) -> Self {
        self.max_buffered_responses = max_buffered_responses;
        self
    }

    /// Build [ConcurrentDownloader] with provided consensus
    /// and header client implementations
    pub fn build<C: Consensus, H: HeadersClient>(
        self,
        consensus: Arc<C>,
        client: Arc<H>,
        local_head: SealedHeader,
        next_chain_tip_hash: H256,
        next_chain_tip_block_number: u64,
    ) -> ConcurrentDownloader<C, H> {
        let Self {
            request_batch_size,
            request_retries,
            stream_batch_size,
            max_concurrent_requests,
            max_buffered_responses,
        } = self;
        ConcurrentDownloader {
            consensus,
            client,
            local_head,
            next_request_block_number: next_chain_tip_block_number,
            lowest_validated_header: None,
            next_chain_tip_block_number,
            next_chain_tip_hash,
            request_batch_size,
            request_retries,
            max_concurrent_requests,
            stream_batch_size,
            max_buffered_responses,
            in_progress_queue: Default::default(),
            buffered_responses: Default::default(),
            queued_validated_headers: Default::default(),
            is_terminated: false,
        }
    }
}

/// Configures and returns the next [HeadersRequest] based on the given parameters
///
/// The request wil start at the given `next_request_block_number` block.
/// The `limit` of the request will either be the targeted `request_batch_size` or the difference of
/// `next_request_block_number` and the `local_head` in case this is smaller than the targeted
/// `request_batch_size`.
#[inline]
fn calc_next_request(
    local_head: u64,
    next_request_block_number: u64,
    request_batch_size: u64,
) -> HeadersRequest {
    // downloading is in reverse
    let diff = next_request_block_number - local_head;
    let limit = diff.min(request_batch_size);
    let start = next_request_block_number;
    HeadersRequest { start: start.into(), limit, direction: HeadersDirection::Falling }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_request_calc() {
        // request an entire batch
        let local = 0;
        let next = 1000;
        let batch_size = 2;
        let request = calc_next_request(local, next, batch_size);
        assert_eq!(request.start, next.into());
        assert_eq!(request.limit, batch_size);

        // only request 1
        let local = 999;
        let next = 1000;
        let batch_size = 2;
        let request = calc_next_request(local, next, batch_size);
        assert_eq!(request.start, next.into());
        assert_eq!(request.limit, 1);
    }

    /// Tests that request calc works
    #[test]
    fn test_next_request() {
        let client = Arc::new(TestHeadersClient::default());

        let genesis = SealedHeader::default();

        let batch_size = 99;
        let start = 1000;
        let mut downloader = ConcurrentDownloadBuilder::default()
            .request_batch_size(batch_size)
            .build(CONSENSUS.clone(), Arc::clone(&client), genesis, H256::random(), start);

        let mut total = 0;
        while let Some(req) = downloader.next_request() {
            assert_eq!(req.start, (start - total).into());
            total += req.limit;
        }
        assert_eq!(total, start);
        assert_eq!(downloader.next_request_block_number, downloader.local_block_number());
    }

    #[test]
    fn test_resp_order() {
        let mut heap = BinaryHeap::new();
        let lo = 0u64;
        heap.push(OrderedHeadersResponse {
            headers: vec![],
            request: HeadersRequest { start: lo.into(), limit: 0, direction: Default::default() },
            retries: 0,
            peer_id: Default::default(),
        });

        let hi = 1u64;
        heap.push(OrderedHeadersResponse {
            headers: vec![],
            request: HeadersRequest { start: hi.into(), limit: 0, direction: Default::default() },
            retries: 0,
            peer_id: Default::default(),
        });

        assert_eq!(heap.pop().unwrap().block_number(), hi);
        assert_eq!(heap.pop().unwrap().block_number(), lo);
    }

    #[tokio::test]
    async fn stream_empty() {
        let local = SealedHeader::default();

        let client = Arc::new(TestHeadersClient::default());
        let mut downloader = ConcurrentDownloadBuilder::default().build(
            CONSENSUS.clone(),
            Arc::clone(&client),
            local,
            H256::random(),
            100,
        );

        let result = downloader.next().await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn download_at_fork_head() {
        reth_tracing::init_test_tracing();

        let client = Arc::new(TestHeadersClient::default());

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        let mut downloader = ConcurrentDownloadBuilder::default()
            .stream_batch_size(3)
            .request_batch_size(3)
            .build(CONSENSUS.clone(), Arc::clone(&client), p3.clone(), p0.hash(), p0.number);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let headers = downloader.next().await.unwrap().unwrap();
        assert_eq!(headers, vec![p0, p1, p2,]);
        assert!(downloader.buffered_responses.is_empty());
        assert!(downloader.next().await.is_none());
        assert!(downloader.next().await.is_none());
    }

    #[tokio::test]
    async fn download_one_by_one() {
        reth_tracing::init_test_tracing();
        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        let client = Arc::new(TestHeadersClient::default());
        let mut downloader = ConcurrentDownloadBuilder::default()
            .stream_batch_size(1)
            .request_batch_size(1)
            .build(CONSENSUS.clone(), Arc::clone(&client), p3.clone(), p0.hash(), p0.number);

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let headers = downloader.next().await.unwrap().unwrap();
        assert_eq!(headers, vec![p0]);

        let headers = downloader.next().await.unwrap().unwrap();
        assert_eq!(headers, vec![p1]);
        let headers = downloader.next().await.unwrap().unwrap();
        assert_eq!(headers, vec![p2]);
        assert!(downloader.next().await.is_none());
    }
}
