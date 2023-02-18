//! A headers downloader that can handle multiple requests concurrently.

use super::task::TaskDownloader;
use crate::metrics::DownloaderMetrics;
use futures::{stream::Stream, FutureExt};
use futures_util::{stream::FuturesUnordered, StreamExt};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        error::{DownloadError, DownloadResult, PeerRequestResult},
        headers::{
            client::{HeadersClient, HeadersRequest},
            downloader::{validate_header_download, HeaderDownloader, SyncTarget},
        },
        priority::Priority,
    },
};
use reth_primitives::{BlockNumber, Header, HeadersDirection, PeerId, SealedHeader, H256};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use std::{
    cmp::{Ordering, Reverse},
    collections::{binary_heap::PeekMut, BinaryHeap},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tracing::trace;

/// A heuristic that is used to determine the number of requests that should be prepared for a peer.
/// This should ensure that there are always requests lined up for peers to handle while the
/// downloader is yielding a next batch of headers that is being committed to the database.
const REQUESTS_PER_PEER_MULTIPLIER: usize = 5;

/// The scope for headers downloader metrics.
pub const HEADERS_DOWNLOADER_SCOPE: &str = "downloaders.headers";

/// Downloads headers concurrently.
///
/// This [HeaderDownloader] downloads headers using the configured [HeadersClient].
/// Headers can be requested by hash or block number and take a `limit` parameter. This downloader
/// tries to fill the gap between the local head of the node and the chain tip by issuing multiple
/// requests at a time but yielding them in batches on [Stream::poll_next].
///
/// **Note:** This downloader downloads in reverse, see also [HeadersDirection::Falling], this means
/// the batches of headers that this downloader yields will start at the chain tip and move towards
/// the local head: falling block numbers.
#[must_use = "Stream does nothing unless polled"]
pub struct ReverseHeadersDownloader<H: HeadersClient> {
    /// Consensus client used to validate headers
    consensus: Arc<dyn Consensus>,
    /// Client used to download headers.
    client: Arc<H>,
    /// The local head of the chain.
    local_head: Option<SealedHeader>,
    /// Block we want to close the gap to.
    sync_target: Option<SyncTargetBlock>,
    /// The block number to use for requests.
    next_request_block_number: u64,
    /// Keeps track of the block we need to validate next.
    lowest_validated_header: Option<SealedHeader>,
    /// Tip block number to start validating from (in reverse)
    next_chain_tip_block_number: u64,
    /// The batch size per one request
    request_limit: u64,
    /// Minimum amount of requests to handle concurrently.
    min_concurrent_requests: usize,
    /// Maximum amount of requests to handle concurrently.
    max_concurrent_requests: usize,
    /// The number of block headers to return at once
    stream_batch_size: usize,
    /// Maximum amount of received headers to buffer internally.
    max_buffered_responses: usize,
    /// Contains the request to retrieve the headers for the sync target
    ///
    /// This will give us the block number of the `sync_target`, after which we can send multiple
    /// requests at a time.
    sync_target_request: Option<HeadersRequestFuture<H::Output>>,
    /// requests in progress
    in_progress_queue: FuturesUnordered<HeadersRequestFuture<H::Output>>,
    /// Buffered, unvalidated responses
    buffered_responses: BinaryHeap<OrderedHeadersResponse>,
    /// Buffered, _sorted_ and validated headers ready to be returned.
    ///
    /// Note: headers are sorted from high to low
    queued_validated_headers: Vec<SealedHeader>,
    /// Header downloader metrics.
    metrics: DownloaderMetrics,
}

// === impl ReverseHeadersDownloader ===

impl<H> ReverseHeadersDownloader<H>
where
    H: HeadersClient + 'static,
{
    /// Convenience method to create a [ReverseHeadersDownloaderBuilder] without importing it
    pub fn builder() -> ReverseHeadersDownloaderBuilder {
        ReverseHeadersDownloaderBuilder::default()
    }

    /// Returns the block number the local node is at.
    #[inline]
    fn local_block_number(&self) -> Option<BlockNumber> {
        self.local_head.as_ref().map(|h| h.number)
    }

    /// Returns the existing local head block number
    ///
    /// # Panics
    ///
    /// If the local head has not been set.
    #[inline]
    fn existing_local_block_number(&self) -> BlockNumber {
        self.local_head.as_ref().expect("is initialized").number
    }

    /// Returns the existing sync target hash.
    ///
    /// # Panics
    ///
    /// If the sync target has never been set.
    #[inline]
    fn existing_sync_target_hash(&self) -> H256 {
        self.sync_target.as_ref().expect("is initialized").hash
    }

    /// Max requests to handle at the same time
    ///
    /// This depends on the number of active peers but will always be
    /// [`min_concurrent_requests`..`max_concurrent_requests`]
    #[inline]
    fn concurrent_request_limit(&self) -> usize {
        let num_peers = self.client.num_connected_peers();

        // we try to keep more requests than available peers active so that there's always a
        // followup request available for a peer
        let dynamic_target = num_peers * REQUESTS_PER_PEER_MULTIPLIER;
        let max_dynamic = dynamic_target.max(self.min_concurrent_requests);

        // If only a few peers are connected we keep it low
        if num_peers < self.min_concurrent_requests {
            return max_dynamic
        }

        max_dynamic.min(self.max_concurrent_requests)
    }

    /// Returns the next header request
    ///
    /// This will advance the current block towards the local head.
    ///
    /// Returns `None` if no more requests are required.
    fn next_request(&mut self) -> Option<HeadersRequest> {
        if let Some(local_head) = self.local_block_number() {
            if self.next_request_block_number > local_head {
                let request = calc_next_request(
                    local_head,
                    self.next_request_block_number,
                    self.request_limit,
                );
                // need to shift the tracked request block number based on the number of requested
                // headers so follow-up requests will use that as start.
                self.next_request_block_number -= request.limit;

                return Some(request)
            }
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
    /// Caution: this expects the `headers` to be sorted with _falling_ block numbers
    #[allow(clippy::result_large_err)]
    fn process_next_headers(
        &mut self,
        request: HeadersRequest,
        headers: Vec<Header>,
        peer_id: PeerId,
    ) -> Result<(), HeadersResponseError> {
        let sync_target_hash = self.existing_sync_target_hash();
        let mut validated = Vec::with_capacity(headers.len());

        for parent in headers {
            let parent = parent.seal_slow();

            // Validate that the header is the parent header of the last validated header.
            if let Some(validated_header) =
                validated.last().or_else(|| self.lowest_validated_header())
            {
                if let Err(error) = self.validate(validated_header, &parent) {
                    trace!(target: "downloaders::headers", ?error ,"Failed to validate header");
                    return Err(HeadersResponseError { request, peer_id: Some(peer_id), error })
                }
            } else if parent.hash() != sync_target_hash {
                return Err(HeadersResponseError {
                    request,
                    peer_id: Some(peer_id),
                    error: DownloadError::InvalidTip {
                        received: parent.hash(),
                        expected: sync_target_hash,
                    },
                })
            }

            validated.push(parent);
        }

        // update tracked block info (falling block number)
        self.next_chain_tip_block_number =
            validated.last().expect("exists").number.saturating_sub(1);
        self.queued_validated_headers.extend(validated);

        Ok(())
    }

    /// Updates the state based on the given `target_block_number`
    ///
    /// There are three different outcomes:
    ///  * This is the first time this is called: current `sync_target` block is still `None`. In
    ///    which case we're initializing the request trackers to `next_block`
    ///  * The `target_block_number` is _higher_ than the current target. In which case we start
    ///    over with a new range
    ///  * The `target_block_number` is _lower_ than the current target or the _same_. In which case
    ///    we don't need to update the request trackers but need to ensure already buffered headers
    ///    are _not_ higher than the new `target_block_number`.
    fn on_block_number_update(&mut self, target_block_number: u64, next_block: u64) {
        // Update the trackers
        if let Some(old_target) =
            self.sync_target.as_mut().and_then(|t| t.number.replace(target_block_number))
        {
            if target_block_number > old_target {
                // the new target is higher than the old target we need to update the
                // request tracker and reset everything
                self.next_request_block_number = next_block;
                self.next_chain_tip_block_number = next_block;
                self.clear();
            } else {
                // ensure already validated headers are in range
                let skip = self
                    .queued_validated_headers
                    .iter()
                    .take_while(|last| last.number > target_block_number)
                    .count();
                // removes all headers that are higher than current target
                self.queued_validated_headers.drain(..skip);
            }
        } else {
            // this occurs on the initial sync target request
            self.next_request_block_number = next_block;
            self.next_chain_tip_block_number = next_block;
        }
    }

    /// Handles the response for the request for the sync target
    #[allow(clippy::result_large_err)]
    fn on_sync_target_outcome(
        &mut self,
        response: HeadersRequestOutcome,
    ) -> Result<(), HeadersResponseError> {
        let sync_target_hash = self.existing_sync_target_hash();
        let HeadersRequestOutcome { request, outcome } = response;
        match outcome {
            Ok(res) => {
                let (peer_id, mut headers) = res.split();

                // update total downloaded metric
                self.metrics.total_downloaded.increment(headers.len() as u64);

                // sort headers from highest to lowest block number
                headers.sort_unstable_by_key(|h| Reverse(h.number));

                if headers.is_empty() {
                    return Err(HeadersResponseError {
                        request,
                        peer_id: Some(peer_id),
                        error: DownloadError::EmptyResponse,
                    })
                }

                let target = headers.remove(0).seal_slow();

                if target.hash() != sync_target_hash {
                    return Err(HeadersResponseError {
                        request,
                        peer_id: Some(peer_id),
                        error: DownloadError::InvalidTip {
                            received: target.hash(),
                            expected: sync_target_hash,
                        },
                    })
                }

                trace!(target: "downloaders::headers", head=?self.local_block_number(), hash=?target.hash(), number=%target.number, "Received sync target");

                // This is the next block we need to start issuing requests from
                let parent_block_number = target.number.saturating_sub(1);
                self.on_block_number_update(target.number, parent_block_number);

                self.queued_validated_headers.push(target);

                // try to validate all buffered responses blocked by this successful response
                self.try_validate_buffered().map(Err::<(), HeadersResponseError>).transpose()?;

                Ok(())
            }
            Err(err) => Err(HeadersResponseError { request, peer_id: None, error: err.into() }),
        }
    }

    /// Invoked when we received a response
    #[allow(clippy::result_large_err)]
    fn on_headers_outcome(
        &mut self,
        response: HeadersRequestOutcome,
    ) -> Result<(), HeadersResponseError> {
        let requested_block_number = response.block_number();
        let HeadersRequestOutcome { request, outcome } = response;

        match outcome {
            Ok(res) => {
                let (peer_id, mut headers) = res.split();

                // update total downloaded metric
                self.metrics.total_downloaded.increment(headers.len() as u64);

                trace!(target: "downloaders::headers", len=%headers.len(), "Received headers response");

                // sort headers from highest to lowest block number
                headers.sort_unstable_by_key(|h| Reverse(h.number));

                if headers.is_empty() {
                    return Err(HeadersResponseError {
                        request,
                        peer_id: Some(peer_id),
                        error: DownloadError::EmptyResponse,
                    })
                }

                // validate the response
                let highest = &headers[0];

                trace!(target: "downloaders::headers", requested_block_number, highest=?highest.number, "Validating non-empty headers response");

                if highest.number != requested_block_number {
                    return Err(HeadersResponseError {
                        request,
                        peer_id: Some(peer_id),
                        error: DownloadError::HeadersResponseStartBlockMismatch {
                            received: highest.number,
                            expected: requested_block_number,
                        },
                    })
                }

                if (headers.len() as u64) != request.limit {
                    return Err(HeadersResponseError {
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
                    self.process_next_headers(request, headers, peer_id)?;
                    // try to validate all buffered responses blocked by this successful response
                    self.try_validate_buffered()
                        .map(Err::<(), HeadersResponseError>)
                        .transpose()?;
                } else if highest.number > self.existing_local_block_number() {
                    self.metrics.buffered_responses.increment(1.);
                    // can't validate yet
                    self.buffered_responses.push(OrderedHeadersResponse {
                        headers,
                        request,
                        peer_id,
                    })
                }

                Ok(())
            }
            // most likely a noop, because this error
            // would've been handled by the fetcher internally
            Err(err) => {
                trace!(target: "downloaders::headers", %err, "Response error");
                Err(HeadersResponseError { request, peer_id: None, error: err.into() })
            }
        }
    }

    fn penalize_peer(&self, peer_id: Option<PeerId>, error: &DownloadError) {
        // Penalize the peer for bad response
        if let Some(peer_id) = peer_id {
            trace!(target: "downloaders::headers", ?peer_id, ?error, "Penalizing peer");
            self.client.report_bad_message(peer_id);
        }
    }

    /// Handles the error of a bad response
    ///
    /// This will re-submit the request.
    fn on_headers_error(&mut self, err: HeadersResponseError) {
        let HeadersResponseError { request, peer_id, error } = err;

        self.penalize_peer(peer_id, &error);

        // Update error metric
        self.metrics.increment_errors(&error);

        // Re-submit the request
        self.submit_request(request, Priority::High);
    }

    /// Attempts to validate the buffered responses
    ///
    /// Returns an error if the next expected response was popped, but failed validation.
    fn try_validate_buffered(&mut self) -> Option<HeadersResponseError> {
        loop {
            // Check to see if we've already received the next value
            let next_response = self.buffered_responses.peek_mut()?;
            let next_block_number = next_response.block_number();
            match next_block_number.cmp(&self.next_chain_tip_block_number) {
                Ordering::Less => return None,
                Ordering::Equal => {
                    let OrderedHeadersResponse { headers, request, peer_id } =
                        PeekMut::pop(next_response);
                    self.metrics.buffered_responses.decrement(1.);

                    if let Err(err) = self.process_next_headers(request, headers, peer_id) {
                        return Some(err)
                    }
                }
                Ordering::Greater => {
                    self.metrics.buffered_responses.decrement(1.);
                    PeekMut::pop(next_response);
                }
            }
        }
    }

    /// Returns the request for the `sync_target` header.
    fn get_sync_target_request(&self, start: H256) -> HeadersRequest {
        HeadersRequest { start: start.into(), limit: 1, direction: HeadersDirection::Falling }
    }

    /// Starts a request future
    fn submit_request(&mut self, request: HeadersRequest, priority: Priority) {
        trace!(target: "downloaders::headers", ?request, "Submitting headers request");
        self.in_progress_queue.push(self.request_fut(request, priority));
        self.metrics.in_flight_requests.increment(1.);
    }

    fn request_fut(
        &self,
        request: HeadersRequest,
        priority: Priority,
    ) -> HeadersRequestFuture<H::Output> {
        let client = Arc::clone(&self.client);
        HeadersRequestFuture {
            request: Some(request.clone()),
            fut: client.get_headers_with_priority(request, priority),
        }
    }

    /// Validate whether the header is valid in relation to it's parent
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
        validate_header_download(&self.consensus, header, parent)
    }

    /// Clears all requests/responses.
    fn clear(&mut self) {
        self.lowest_validated_header.take();
        self.queued_validated_headers.clear();
        self.buffered_responses.clear();
        self.in_progress_queue.clear();

        self.metrics.in_flight_requests.set(0.);
        self.metrics.buffered_responses.set(0.);
    }

    /// Splits off the next batch of headers
    fn split_next_batch(&mut self) -> Vec<SealedHeader> {
        let batch_size = self.stream_batch_size.min(self.queued_validated_headers.len());
        let mut rem = self.queued_validated_headers.split_off(batch_size);
        std::mem::swap(&mut rem, &mut self.queued_validated_headers);
        rem
    }
}

impl<H> ReverseHeadersDownloader<H>
where
    H: HeadersClient,
    Self: HeaderDownloader + 'static,
{
    /// Spawns the downloader task via [tokio::task::spawn]
    pub fn into_task(self) -> TaskDownloader {
        self.into_task_with(&TokioTaskExecutor::default())
    }

    /// Convert the downloader into a [`TaskDownloader`](super::task::TaskDownloader) by spawning
    /// it via the given `spawner`.
    pub fn into_task_with<S>(self, spawner: &S) -> TaskDownloader
    where
        S: TaskSpawner,
    {
        TaskDownloader::spawn_with(self, spawner)
    }
}

impl<H> HeaderDownloader for ReverseHeadersDownloader<H>
where
    H: HeadersClient + 'static,
{
    fn update_local_head(&mut self, head: SealedHeader) {
        // ensure we're only yielding headers that are in range and follow the current local head.
        while self
            .queued_validated_headers
            .last()
            .map(|last| last.number <= head.number)
            .unwrap_or_default()
        {
            // headers are sorted high to low
            self.queued_validated_headers.pop();
        }
        // update the local head
        self.local_head = Some(head);
    }

    /// If the given target is different from the current target, we need to update the sync target
    fn update_sync_target(&mut self, target: SyncTarget) {
        let current_tip = self.sync_target.as_ref().map(|t| t.hash);
        match target {
            SyncTarget::Tip(tip) => {
                if Some(tip) != current_tip {
                    trace!(target: "downloaders::headers", current=?current_tip, new=?tip, "Update sync target");
                    let new_sync_target = SyncTargetBlock::from_hash(tip);

                    // if the new sync target is the next queued request we don't need to re-start
                    // the target update
                    if let Some(target_number) = self
                        .queued_validated_headers
                        .first()
                        .filter(|h| h.hash() == tip)
                        .map(|h| h.number)
                    {
                        self.sync_target = Some(new_sync_target.with_number(target_number));
                        return
                    }

                    trace!(target: "downloaders::headers", new=?target, "Request new sync target");
                    self.metrics.out_of_order_requests.increment(1);
                    self.sync_target = Some(new_sync_target);
                    self.sync_target_request =
                        Some(self.request_fut(self.get_sync_target_request(tip), Priority::High));
                }
            }
            SyncTarget::Gap(existing) => {
                let target = existing.parent_hash;
                if Some(target) != current_tip {
                    // there could be a sync target request in progress
                    self.sync_target_request.take();
                    // If the target has changed, update the request pointers based on the new
                    // targeted block number
                    let parent_block_number = existing.number.saturating_sub(1);

                    trace!(target: "downloaders::headers", current=?current_tip, new=?target, %parent_block_number, "Updated sync target");

                    // Update the sync target hash
                    self.sync_target = match self.sync_target.take() {
                        Some(mut sync_target) => {
                            sync_target.hash = target;
                            Some(sync_target)
                        }
                        None => Some(SyncTargetBlock::from_hash(target)),
                    };
                    self.on_block_number_update(parent_block_number, parent_block_number);
                }
            }
        }
    }

    fn set_batch_size(&mut self, batch_size: usize) {
        self.stream_batch_size = batch_size;
    }
}

impl<H> Stream for ReverseHeadersDownloader<H>
where
    H: HeadersClient + 'static,
{
    type Item = Vec<SealedHeader>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // The downloader boundaries (local head and sync target) have to be set in order
        // to start downloading data.
        if this.local_head.is_none() || this.sync_target.is_none() {
            trace!(
                target: "downloaders::headers",
                head=?this.local_block_number(),
                sync_target=?this.sync_target,
                "The downloader sync boundaries have not been set"
            );
            return Poll::Pending
        }

        // If we have a new tip request we need to complete that first before we send batched
        // requests
        while let Some(mut req) = this.sync_target_request.take() {
            match req.poll_unpin(cx) {
                Poll::Ready(outcome) => {
                    if let Err(err) = this.on_sync_target_outcome(outcome) {
                        if err.is_channel_closed() {
                            // download channel closed which means the network was dropped
                            return Poll::Ready(None)
                        }

                        this.penalize_peer(err.peer_id, &err.error);
                        this.metrics.increment_errors(&err.error);
                        this.sync_target_request =
                            Some(this.request_fut(err.request, Priority::High));
                    } else {
                        break
                    }
                }
                Poll::Pending => {
                    this.sync_target_request = Some(req);
                    return Poll::Pending
                }
            }
        }

        // this loop will submit new requests and poll them, if a new batch is ready it is returned
        // The actual work is done by the receiver of the request channel, this means, polling the
        // request future is just reading from a `oneshot::Receiver`. Hence, this loop tries to keep
        // the downloader at capacity at all times The order of loops is as follows:
        // 1. poll futures to make room for followup requests (this will also prepare validated
        // headers for 3.) 2. exhaust all capacity by sending requests
        // 3. return batch, if enough validated
        // 4. return Pending if 2.) did not submit a new request, else continue
        loop {
            // poll requests
            while let Poll::Ready(Some(outcome)) = this.in_progress_queue.poll_next_unpin(cx) {
                this.metrics.in_flight_requests.decrement(1.);
                // handle response
                if let Err(err) = this.on_headers_outcome(outcome) {
                    if err.is_channel_closed() {
                        // download channel closed which means the network was dropped
                        return Poll::Ready(None)
                    }
                    this.on_headers_error(err);
                }
            }

            // marks the loop's exit condition: exit if no requests submitted
            let mut progress = false;

            let concurrent_request_limit = this.concurrent_request_limit();
            // populate requests
            while this.in_progress_queue.len() < concurrent_request_limit &&
                this.buffered_responses.len() < this.max_buffered_responses
            {
                if let Some(request) = this.next_request() {
                    trace!(
                        target: "downloaders::headers",
                        "Requesting headers {request:?}"
                    );
                    progress = true;
                    this.submit_request(request, Priority::Normal);
                } else {
                    // no more requests
                    break
                }
            }

            // yield next batch
            if this.queued_validated_headers.len() >= this.stream_batch_size {
                let next_batch = this.split_next_batch();

                // Note: if this would drain all headers, we need to keep the lowest (last index)
                // around so we can continue validating headers responses.
                if this.queued_validated_headers.is_empty() {
                    this.lowest_validated_header = next_batch.last().cloned();
                }

                trace!(target: "downloaders::headers", batch=%next_batch.len(), "Returning validated batch");

                this.metrics.total_flushed.increment(next_batch.len() as u64);
                return Poll::Ready(Some(next_batch))
            }

            if !progress {
                break
            }
        }

        // all requests are handled, stream is finished
        if this.in_progress_queue.is_empty() {
            let next_batch = this.split_next_batch();
            if next_batch.is_empty() {
                this.clear();
                return Poll::Ready(None)
            }
            this.metrics.total_flushed.increment(next_batch.len() as u64);
            return Poll::Ready(Some(next_batch))
        }

        Poll::Pending
    }
}

/// A future that returns a list of [`Header`] on success.
struct HeadersRequestFuture<F> {
    request: Option<HeadersRequest>,
    fut: F,
}

impl<F> Future for HeadersRequestFuture<F>
where
    F: Future<Output = PeerRequestResult<Vec<Header>>> + Sync + Send + Unpin,
{
    type Output = HeadersRequestOutcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let outcome = ready!(this.fut.poll_unpin(cx));
        let request = this.request.take().unwrap();

        Poll::Ready(HeadersRequestOutcome { request, outcome })
    }
}

/// The outcome of the [HeadersRequestFuture]
struct HeadersRequestOutcome {
    request: HeadersRequest,
    outcome: PeerRequestResult<Vec<Header>>,
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
#[derive(Debug)]
struct HeadersResponseError {
    request: HeadersRequest,
    peer_id: Option<PeerId>,
    error: DownloadError,
}

impl HeadersResponseError {
    /// Returns true if the error was caused by a closed channel to the network.
    fn is_channel_closed(&self) -> bool {
        if let DownloadError::RequestError(ref err) = self.error {
            return err.is_channel_closed()
        }
        false
    }
}

/// The block to which we want to close the gap: (local head...sync target]
#[derive(Debug, Default)]
struct SyncTargetBlock {
    /// Block hash of the targeted block
    hash: H256,
    /// This is an `Option` because we don't know the block number at first
    number: Option<u64>,
}

impl SyncTargetBlock {
    /// Create new instance from hash.
    fn from_hash(hash: H256) -> Self {
        Self { hash, number: None }
    }

    /// Set a number on the instance.
    fn with_number(mut self, number: u64) -> Self {
        self.number = Some(number);
        self
    }
}

/// The builder for [ReverseHeadersDownloader] with
/// some default settings
#[derive(Debug)]
pub struct ReverseHeadersDownloaderBuilder {
    /// The batch size per one request
    request_limit: u64,
    /// Batch size for headers
    stream_batch_size: usize,
    /// Batch size for headers
    min_concurrent_requests: usize,
    /// Batch size for headers
    max_concurrent_requests: usize,
    /// How many responses to buffer
    max_buffered_responses: usize,
}

impl Default for ReverseHeadersDownloaderBuilder {
    fn default() -> Self {
        Self {
            request_limit: 1_000,
            stream_batch_size: 10_000,
            max_concurrent_requests: 150,
            min_concurrent_requests: 5,
            max_buffered_responses: 750,
        }
    }
}

impl ReverseHeadersDownloaderBuilder {
    /// Set the request batch size.
    ///
    /// This determines the `limit` for a [GetHeaders](reth_eth_wire::GetBlockHeaders) requests, the
    /// number of headers we ask for.
    pub fn request_limit(mut self, limit: u64) -> Self {
        self.request_limit = limit;
        self
    }

    /// Set the stream batch size
    ///
    /// This determines the number of headers the [ReverseHeadersDownloader] will yield on
    /// `Stream::next`. This will be the amount of headers the headers stage will commit at a
    /// time.
    pub fn stream_batch_size(mut self, size: usize) -> Self {
        self.stream_batch_size = size;
        self
    }

    /// Set the min amount of concurrent requests.
    ///
    /// If there's capacity the [ReverseHeadersDownloader] will keep at least this many requests
    /// active at a time.
    pub fn min_concurrent_requests(mut self, min_concurrent_requests: usize) -> Self {
        self.min_concurrent_requests = min_concurrent_requests;
        self
    }

    /// Set the max amount of concurrent requests.
    ///
    /// The downloader's concurrent requests won't exceed the given amount.
    pub fn max_concurrent_requests(mut self, max_concurrent_requests: usize) -> Self {
        self.max_concurrent_requests = max_concurrent_requests;
        self
    }

    /// How many responses to buffer internally.
    ///
    /// This essentially determines how much memory the downloader can use for buffering responses
    /// that arrive out of order. The total number of buffered headers is `request_limit *
    /// max_buffered_responses`. If the [ReverseHeadersDownloader]'s buffered responses exceeds this
    /// threshold it waits until there's capacity again before sending new requests.
    pub fn max_buffered_responses(mut self, max_buffered_responses: usize) -> Self {
        self.max_buffered_responses = max_buffered_responses;
        self
    }

    /// Build [ReverseHeadersDownloader] with provided consensus
    /// and header client implementations
    pub fn build<H>(
        self,
        client: Arc<H>,
        consensus: Arc<dyn Consensus>,
    ) -> ReverseHeadersDownloader<H>
    where
        H: HeadersClient + 'static,
    {
        let Self {
            request_limit,
            stream_batch_size,
            min_concurrent_requests,
            max_concurrent_requests,
            max_buffered_responses,
        } = self;
        ReverseHeadersDownloader {
            consensus,
            client,
            local_head: None,
            sync_target: None,
            // Note: we set these to `0` first, they'll be updated once the sync target response is
            // handled and only used afterwards
            next_request_block_number: 0,
            next_chain_tip_block_number: 0,
            lowest_validated_header: None,
            request_limit,
            min_concurrent_requests,
            max_concurrent_requests,
            stream_batch_size,
            max_buffered_responses,
            sync_target_request: None,
            in_progress_queue: Default::default(),
            buffered_responses: Default::default(),
            queued_validated_headers: Default::default(),
            metrics: DownloaderMetrics::new(HEADERS_DOWNLOADER_SCOPE),
        }
    }
}

/// Configures and returns the next [HeadersRequest] based on the given parameters
///
/// The request wil start at the given `next_request_block_number` block.
/// The `limit` of the request will either be the targeted `request_limit` or the difference of
/// `next_request_block_number` and the `local_head` in case this is smaller than the targeted
/// `request_limit`.
#[inline]
fn calc_next_request(
    local_head: u64,
    next_request_block_number: u64,
    request_limit: u64,
) -> HeadersRequest {
    // downloading is in reverse
    let diff = next_request_block_number - local_head;
    let limit = diff.min(request_limit);
    let start = next_request_block_number;
    HeadersRequest { start: start.into(), limit, direction: HeadersDirection::Falling }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::headers::test_utils::child_header;
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::{TestConsensus, TestHeadersClient};
    use reth_primitives::SealedHeader;

    /// Tests that request calc works
    #[test]
    fn test_sync_target_update() {
        let client = Arc::new(TestHeadersClient::default());

        let genesis = SealedHeader::default();

        let mut downloader = ReverseHeadersDownloaderBuilder::default()
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        downloader.update_local_head(genesis);
        downloader.update_sync_target(SyncTarget::Tip(H256::random()));

        downloader.sync_target_request.take();

        let target = SyncTarget::Tip(H256::random());
        downloader.update_sync_target(target);
        assert!(downloader.sync_target_request.is_some());

        downloader.sync_target_request.take();
        let target = SyncTarget::Gap(Header::default().seal(H256::random()));
        downloader.update_sync_target(target);
        assert!(downloader.sync_target_request.is_none());
        assert_matches!(
            downloader.sync_target,
            Some(target) => target.number.is_some()
        );
    }

    /// Tests that request calc works
    #[test]
    fn test_head_update() {
        let client = Arc::new(TestHeadersClient::default());

        let header = SealedHeader::default();

        let mut downloader = ReverseHeadersDownloaderBuilder::default()
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        downloader.update_local_head(header.clone());
        downloader.update_sync_target(SyncTarget::Tip(H256::random()));

        downloader.queued_validated_headers.push(header.clone());
        let mut next = header.as_ref().clone();
        next.number += 1;
        downloader.update_local_head(next.seal(H256::random()));
        assert!(downloader.queued_validated_headers.is_empty());
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
        let mut downloader = ReverseHeadersDownloaderBuilder::default()
            .request_limit(batch_size)
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        downloader.update_local_head(genesis);
        downloader.update_sync_target(SyncTarget::Tip(H256::random()));

        downloader.next_request_block_number = start;

        let mut total = 0;
        while let Some(req) = downloader.next_request() {
            assert_eq!(req.start, (start - total).into());
            total += req.limit;
        }
        assert_eq!(total, start);
        assert_eq!(Some(downloader.next_request_block_number), downloader.local_block_number());
    }

    #[test]
    fn test_resp_order() {
        let mut heap = BinaryHeap::new();
        let hi = 1u64;
        heap.push(OrderedHeadersResponse {
            headers: vec![],
            request: HeadersRequest { start: hi.into(), limit: 0, direction: Default::default() },
            peer_id: Default::default(),
        });

        let lo = 0u64;
        heap.push(OrderedHeadersResponse {
            headers: vec![],
            request: HeadersRequest { start: lo.into(), limit: 0, direction: Default::default() },
            peer_id: Default::default(),
        });

        assert_eq!(heap.pop().unwrap().block_number(), hi);
        assert_eq!(heap.pop().unwrap().block_number(), lo);
    }

    #[tokio::test]
    async fn download_at_fork_head() {
        reth_tracing::init_test_tracing();

        let client = Arc::new(TestHeadersClient::default());

        let p3 = SealedHeader::default();
        let p2 = child_header(&p3);
        let p1 = child_header(&p2);
        let p0 = child_header(&p1);

        let mut downloader = ReverseHeadersDownloaderBuilder::default()
            .stream_batch_size(3)
            .request_limit(3)
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        downloader.update_local_head(p3.clone());
        downloader.update_sync_target(SyncTarget::Tip(p0.hash()));

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let headers = downloader.next().await.unwrap();
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
        let mut downloader = ReverseHeadersDownloaderBuilder::default()
            .stream_batch_size(1)
            .request_limit(1)
            .build(Arc::clone(&client), Arc::new(TestConsensus::default()));
        downloader.update_local_head(p3.clone());
        downloader.update_sync_target(SyncTarget::Tip(p0.hash()));

        client
            .extend(vec![
                p0.as_ref().clone(),
                p1.as_ref().clone(),
                p2.as_ref().clone(),
                p3.as_ref().clone(),
            ])
            .await;

        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, vec![p0]);

        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, vec![p1]);
        let headers = downloader.next().await.unwrap();
        assert_eq!(headers, vec![p2]);
        assert!(downloader.next().await.is_none());
    }
}
