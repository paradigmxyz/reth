use super::queue::BodiesRequestQueue;
use crate::{bodies::task::TaskDownloader, metrics::DownloaderMetrics};
use futures::Stream;
use futures_util::StreamExt;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        bodies::{
            client::BodiesClient,
            downloader::{BodyDownloader, BodyDownloaderResult},
            response::BlockResponse,
        },
        error::{DownloadError, DownloadResult},
        priority::Priority,
    },
};
use reth_primitives::{BlockNumber, SealedHeader};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    ops::{Range, RangeInclusive},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// The multiplier for the number of connected peers.
/// This might be used as an upper bound for determining the
/// number of concurrent requests.
///
/// The multiplier is needed to optimistically increase the number
/// of concurrent requests, since we are expecting to connect to more peers
/// in the near future.
const CONCURRENCY_PEER_MULTIPLIER: usize = 4;

/// The scope for headers downloader metrics.
pub const BODIES_DOWNLOADER_SCOPE: &str = "downloaders.bodies";

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[must_use = "Stream does nothing unless polled"]
#[derive(Debug)]
pub struct BodiesDownloader<B: BodiesClient, DB> {
    /// The bodies client
    client: Arc<B>,
    /// The consensus client
    consensus: Arc<dyn Consensus>,
    // TODO: make this a [HeaderProvider]
    /// The database handle
    db: Arc<DB>,
    /// The maximum number of non-empty blocks per one request
    request_limit: u64,
    /// The maximum number of block bodies returned at once from the stream
    stream_batch_size: usize,
    /// The allowed range for number of concurrent requests.
    concurrent_requests_range: RangeInclusive<usize>,
    /// Maximum amount of received bodies to buffer internally.
    max_buffered_responses: usize,
    /// The range of block numbers for body download.
    download_range: Range<BlockNumber>,
    /// The latest block number returned.
    latest_queued_block_number: Option<BlockNumber>,
    /// Requests in progress
    in_progress_queue: BodiesRequestQueue<B>,
    /// Buffered responses
    buffered_responses: BinaryHeap<OrderedBodiesResponse>,
    /// Queued body responses
    queued_bodies: Vec<BlockResponse>,
    /// The bodies downloader metrics.
    metrics: DownloaderMetrics,
}

impl<B, DB> BodiesDownloader<B, DB>
where
    B: BodiesClient + 'static,
    DB: Database,
{
    /// Returns the next contiguous request.
    fn next_headers_request(&mut self) -> DownloadResult<Option<Vec<SealedHeader>>> {
        let start_at = match self.in_progress_queue.last_requested_block_number {
            Some(num) => num + 1,
            None => self.download_range.start,
        };

        let limit = self.download_range.end.saturating_sub(start_at).min(self.request_limit);
        self.query_headers(start_at..self.download_range.end, limit)
    }

    /// Retrieve a batch of headers from the database starting from provided block number.
    ///
    /// This method is going to return the batch as soon as one of the conditions below
    /// is fullfilled:
    ///     1. The number of non-empty headers in the batch equals requested.
    ///     2. The total number of headers in the batch (both empty and non-empty)
    ///        is greater than or equal to the stream batch size.
    ///     3. Downloader reached the end of the range
    ///
    /// NOTE: The batches returned have a variable length.
    fn query_headers(
        &self,
        range: Range<BlockNumber>,
        max_non_empty: u64,
    ) -> DownloadResult<Option<Vec<SealedHeader>>> {
        if range.is_empty() || max_non_empty == 0 {
            return Ok(None)
        }

        // Acquire cursors over canonical and header tables
        let tx = self.db.tx()?;
        let mut canonical_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;

        // Non empty headers count
        let mut non_empty_headers = 0;

        // Collection of results
        let mut headers = Vec::<SealedHeader>::default();

        let mut current_block_num = range.start;

        // Collect headers while
        //      1. Current block number is in range
        //      2. The number of non empty headers is less than maximum
        //      3. The total number of headers is less than the stream batch size
        while range.contains(&current_block_num) &&
            non_empty_headers < max_non_empty &&
            headers.len() < self.stream_batch_size
        {
            // Find the block hash.
            let (number, hash) = canonical_cursor
                .seek_exact(current_block_num)?
                .ok_or(DownloadError::MissingHeader { block_number: current_block_num })?;
            // Find the block header.
            let (_, header) = header_cursor
                .seek_exact(number)?
                .ok_or(DownloadError::MissingHeader { block_number: number })?;

            // If the header is not empty, increment the counter
            if !header.is_empty() {
                non_empty_headers += 1;
            }

            // Add header to the result collection
            headers.push(header.seal(hash));

            // Increment current block number
            current_block_num += 1;
        }

        Ok(Some(headers).filter(|h| !h.is_empty()))
    }

    /// Get the next expected block number for queueing.
    fn next_expected_block_number(&self) -> BlockNumber {
        match self.latest_queued_block_number {
            Some(num) => num + 1,
            None => self.download_range.start,
        }
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
        let dynamic_target = num_peers * CONCURRENCY_PEER_MULTIPLIER;
        let max_dynamic = dynamic_target.max(*self.concurrent_requests_range.start());

        // If only a few peers are connected we keep it low
        if num_peers < *self.concurrent_requests_range.start() {
            return max_dynamic
        }

        max_dynamic.min(*self.concurrent_requests_range.end())
    }

    // Check if the stream is terminated
    fn is_terminated(&self) -> bool {
        // There is nothing to request if the range is empty
        let nothing_to_request = self.download_range.is_empty() ||
            // or all blocks have already been requested.
            self.in_progress_queue
                .last_requested_block_number
                .map(|last| last + 1 == self.download_range.end)
                .unwrap_or_default();

        nothing_to_request &&
            self.in_progress_queue.is_empty() &&
            self.buffered_responses.is_empty() &&
            self.queued_bodies.is_empty()
    }

    /// Clear all download related data.
    ///
    /// Should be invoked upon encountering fatal error.
    fn clear(&mut self) {
        self.download_range = Range::default();
        self.latest_queued_block_number.take();
        self.in_progress_queue.clear();
        self.buffered_responses.clear();
        self.queued_bodies.clear();

        self.metrics.in_flight_requests.set(0.);
        self.metrics.buffered_responses.set(0.);
    }

    /// Queues bodies and sets the latest queued block number
    fn queue_bodies(&mut self, bodies: Vec<BlockResponse>) {
        self.latest_queued_block_number = Some(bodies.last().expect("is not empty").block_number());
        self.queued_bodies.extend(bodies.into_iter());
    }

    /// Returns a response if it's first block number matches the next expected.
    fn try_next_buffered(&mut self) -> Option<Vec<BlockResponse>> {
        if let Some(next) = self.buffered_responses.peek() {
            let expected = self.next_expected_block_number();
            let next_block_rng = next.block_range();

            if next_block_rng.contains(&expected) {
                return self.buffered_responses.pop().map(|buffered| {
                    self.metrics.buffered_responses.decrement(1.);
                    buffered
                        .0
                        .into_iter()
                        .skip_while(|b| b.block_number() < expected)
                        .take_while(|b| self.download_range.contains(&b.block_number()))
                        .collect()
                })
            }

            // Drop buffered response since we passed that range
            if *next_block_rng.end() < expected {
                self.metrics.buffered_responses.decrement(1.);
                self.buffered_responses.pop();
            }
        }
        None
    }
}

impl<B, DB> BodiesDownloader<B, DB>
where
    B: BodiesClient + 'static,
    DB: Database,
    Self: BodyDownloader + 'static,
{
    /// Spawns the downloader task via [tokio::task::spawn]
    pub fn into_task(self) -> TaskDownloader {
        self.into_task_with(&TokioTaskExecutor::default())
    }

    /// Convert the downloader into a [`TaskDownloader`](super::task::TaskDownloader) by spawning
    /// it via the given spawner.
    pub fn into_task_with<S>(self, spawner: &S) -> TaskDownloader
    where
        S: TaskSpawner,
    {
        TaskDownloader::spawn_with(self, spawner)
    }
}

impl<B, DB> BodyDownloader for BodiesDownloader<B, DB>
where
    B: BodiesClient + 'static,
    DB: Database,
{
    /// Set a new download range (exclusive).
    ///
    /// This method will drain all queued bodies, filter out ones outside the range and put them
    /// back into the buffer.
    /// If there are any bodies between the range start and last queued body that have not been
    /// downloaded or are not in progress, they will be re-requested.
    fn set_download_range(&mut self, range: Range<BlockNumber>) -> DownloadResult<()> {
        // Check if the range is valid.
        if range.is_empty() {
            tracing::error!(target: "downloaders::bodies", ?range, "Range is invalid");
            return Err(DownloadError::InvalidBodyRange { range })
        }

        // Check if the provided range is the subset of the existing range.
        let is_current_range_subset =
            self.download_range.contains(&range.start) && range.end == self.download_range.end;
        if is_current_range_subset {
            tracing::trace!(target: "downloaders::bodies", ?range, "Download range already in progress");
            // The current range already includes requested.
            return Ok(())
        }

        // Check if the provided range is the next expected range.
        let is_next_range = range.start >= self.download_range.end;
        if is_next_range {
            // New range received.
            tracing::trace!(target: "downloaders::bodies", ?range, "New download range set");
            self.download_range = range;
            return Ok(())
        }

        // The block range reset after unwind.
        tracing::trace!(target: "downloaders::bodies", ?range, prev_range = ?self.download_range, "Download range reset");
        self.clear();
        self.download_range = range;
        Ok(())
    }
}

impl<B, DB> Stream for BodiesDownloader<B, DB>
where
    B: BodiesClient + 'static,
    DB: Database,
{
    type Item = BodyDownloaderResult;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.is_terminated() {
            return Poll::Ready(None)
        }

        // Submit new requests and poll any in progress
        loop {
            // Poll requests
            while let Poll::Ready(Some(response)) = this.in_progress_queue.poll_next_unpin(cx) {
                this.metrics.in_flight_requests.decrement(1.);
                match response {
                    Ok(response) => {
                        let response = OrderedBodiesResponse(response);
                        this.buffered_responses.push(response);
                        this.metrics.buffered_responses.increment(1.);
                    }
                    Err(error) => {
                        tracing::error!(target: "downloaders::bodies", ?error, "Request failed");
                        this.clear();
                        return Poll::Ready(Some(Err(error)))
                    }
                };
            }

            // Loop exit condition
            let mut new_request_submitted = false;
            // Submit new requests
            let concurrent_requests_limit = this.concurrent_request_limit();
            'inner: while this.in_progress_queue.len() < concurrent_requests_limit &&
                this.buffered_responses.len() < this.max_buffered_responses
            {
                match this.next_headers_request() {
                    Ok(Some(request)) => {
                        this.metrics.in_flight_requests.increment(1.);
                        this.in_progress_queue.push_new_request(
                            Arc::clone(&this.client),
                            Arc::clone(&this.consensus),
                            request,
                            Priority::Normal,
                        );
                        new_request_submitted = true;
                    }
                    Ok(None) => break 'inner,
                    Err(error) => {
                        tracing::error!(target: "downloaders::bodies", ?error, "Failed to form next request");
                        this.clear();
                        return Poll::Ready(Some(Err(error)))
                    }
                };
            }

            while let Some(buf_response) = this.try_next_buffered() {
                this.queue_bodies(buf_response);
            }

            // Yield next batch
            if this.queued_bodies.len() >= this.stream_batch_size {
                let next_batch = this.queued_bodies.drain(..this.stream_batch_size);
                this.metrics.total_flushed.increment(next_batch.len() as u64);
                return Poll::Ready(Some(Ok(next_batch.collect())))
            }

            if !new_request_submitted {
                break
            }
        }

        // All requests are handled, stream is finished
        if this.in_progress_queue.is_empty() {
            if this.queued_bodies.is_empty() {
                return Poll::Ready(None)
            }

            let batch_size = this.stream_batch_size.min(this.queued_bodies.len());
            let next_batch = this.queued_bodies.drain(..batch_size);
            this.metrics.total_flushed.increment(next_batch.len() as u64);
            return Poll::Ready(Some(Ok(next_batch.collect())))
        }

        Poll::Pending
    }
}

#[derive(Debug)]
struct OrderedBodiesResponse(Vec<BlockResponse>);

impl OrderedBodiesResponse {
    /// Returns the block number of the first element
    ///
    /// # Panics
    /// If the response vec is empty.
    fn first_block_number(&self) -> u64 {
        self.0.first().expect("is not empty").block_number()
    }

    /// Returns the range of the block numbers in the response
    ///
    /// # Panics
    /// If the response vec is empty.
    fn block_range(&self) -> RangeInclusive<u64> {
        self.first_block_number()..=self.0.last().expect("is not empty").block_number()
    }
}

impl PartialEq for OrderedBodiesResponse {
    fn eq(&self, other: &Self) -> bool {
        self.first_block_number() == other.first_block_number()
    }
}

impl Eq for OrderedBodiesResponse {}

impl PartialOrd for OrderedBodiesResponse {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedBodiesResponse {
    fn cmp(&self, other: &Self) -> Ordering {
        self.first_block_number().cmp(&other.first_block_number()).reverse()
    }
}

/// Builder for [BodiesDownloader].
pub struct BodiesDownloaderBuilder {
    /// The batch size of non-empty blocks per one request
    request_limit: u64,
    /// The maximum number of block bodies returned at once from the stream
    stream_batch_size: usize,
    /// Maximum amount of received bodies to buffer internally.
    max_buffered_responses: usize,
    /// The maximum number of requests to send concurrently.
    concurrent_requests_range: RangeInclusive<usize>,
}

impl Default for BodiesDownloaderBuilder {
    fn default() -> Self {
        Self {
            request_limit: 200,
            stream_batch_size: 1000,
            max_buffered_responses: 30000,
            concurrent_requests_range: 5..=100,
        }
    }
}

impl BodiesDownloaderBuilder {
    /// Set request batch size on the downloader.
    pub fn with_request_limit(mut self, request_limit: u64) -> Self {
        self.request_limit = request_limit;
        self
    }

    /// Set stream batch size on the downloader.
    pub fn with_stream_batch_size(mut self, stream_batch_size: usize) -> Self {
        self.stream_batch_size = stream_batch_size;
        self
    }

    /// Set on the downloader.
    pub fn with_concurrent_requests_range(
        mut self,
        concurrent_requests_range: RangeInclusive<usize>,
    ) -> Self {
        self.concurrent_requests_range = concurrent_requests_range;
        self
    }

    /// Set on the downloader.
    pub fn with_max_buffered_responses(mut self, max_buffered_responses: usize) -> Self {
        self.max_buffered_responses = max_buffered_responses;
        self
    }

    /// Consume self and return the concurrent donwloader.
    pub fn build<B, DB>(
        self,
        client: B,
        consensus: Arc<dyn Consensus>,
        db: Arc<DB>,
    ) -> BodiesDownloader<B, DB>
    where
        B: BodiesClient + 'static,
        DB: Database,
    {
        let Self {
            request_limit,
            stream_batch_size,
            concurrent_requests_range,
            max_buffered_responses,
        } = self;
        let metrics = DownloaderMetrics::new(BODIES_DOWNLOADER_SCOPE);
        let in_progress_queue = BodiesRequestQueue::new(metrics.clone());
        BodiesDownloader {
            client: Arc::new(client),
            consensus,
            db,
            request_limit,
            stream_batch_size,
            max_buffered_responses,
            concurrent_requests_range,
            in_progress_queue,
            metrics,
            download_range: Default::default(),
            latest_queued_block_number: None,
            buffered_responses: Default::default(),
            queued_bodies: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::test_utils::{insert_headers, zip_blocks},
        test_utils::{generate_bodies, TestBodiesClient},
    };
    use assert_matches::assert_matches;
    use futures_util::stream::StreamExt;
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_interfaces::test_utils::{generators::random_block_range, TestConsensus};
    use reth_primitives::{BlockBody, H256};
    use std::{collections::HashMap, sync::Arc};

    // Check that the blocks are emitted in order of block number, not in order of
    // first-downloaded
    #[tokio::test]
    async fn streams_bodies_in_order() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..20);

        insert_headers(&db, &headers);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );
        downloader.set_download_range(0..20).expect("failed to set download range");

        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter(), &mut bodies))
        );
        assert_eq!(client.times_requested(), 1);
    }

    // Check that the number of times requested equals to the number of headers divided by request
    // limit.
    #[tokio::test]
    async fn requests_correct_number_of_times() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let blocks = random_block_range(0..200, H256::zero(), 1..2);

        let headers = blocks.iter().map(|block| block.header.clone()).collect::<Vec<_>>();
        let bodies = blocks
            .into_iter()
            .map(|block| {
                (
                    block.hash(),
                    BlockBody {
                        transactions: block.body,
                        ommers: block.ommers.into_iter().map(|header| header.unseal()).collect(),
                        withdrawals: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        insert_headers(&db, &headers);

        let request_limit = 10;
        let client = Arc::new(TestBodiesClient::default().with_bodies(bodies.clone()));
        let mut downloader = BodiesDownloaderBuilder::default()
            .with_request_limit(request_limit)
            .build(client.clone(), Arc::new(TestConsensus::default()), db);
        downloader.set_download_range(0..200).expect("failed to set download range");

        let _ = downloader.collect::<Vec<_>>().await;
        assert_eq!(client.times_requested(), 20);
    }

    // Check that bodies are returned in correct order
    // after resetting the download range multiple times.
    #[tokio::test]
    async fn streams_bodies_in_order_after_range_reset() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..100);

        insert_headers(&db, &headers);

        let stream_batch_size = 20;
        let request_limit = 10;
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let mut downloader = BodiesDownloaderBuilder::default()
            .with_stream_batch_size(stream_batch_size)
            .with_request_limit(request_limit)
            .build(client.clone(), Arc::new(TestConsensus::default()), db);

        let mut range_start = 0;
        while range_start < 100 {
            downloader.set_download_range(range_start..100).expect("failed to set download range");

            assert_matches!(
                downloader.next().await,
                Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().skip(range_start as usize).take(stream_batch_size), &mut bodies))
            );
            assert!(downloader.latest_queued_block_number >= Some(range_start));
            range_start += stream_batch_size as u64;
        }
    }

    // Check that the downloader picks up the new range and downloads bodies after previous range
    // was completed.
    #[tokio::test]
    async fn can_download_new_range_after_termination() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..200);

        insert_headers(&db, &headers);

        let client = Arc::new(TestBodiesClient::default().with_bodies(bodies.clone()));
        let mut downloader = BodiesDownloaderBuilder::default().with_stream_batch_size(100).build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );

        // Set and download the first range
        downloader.set_download_range(0..100).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().take(100), &mut bodies))
        );

        // Check that the stream is terminated
        assert!(downloader.next().await.is_none());

        // Set and download the second range
        downloader.set_download_range(100..200).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().skip(100), &mut bodies))
        );
    }
}
