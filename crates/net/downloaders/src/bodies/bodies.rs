use super::queue::BodiesRequestQueue;
use crate::{bodies::task::TaskDownloader, metrics::BodyDownloaderMetrics};
use alloy_primitives::BlockNumber;
use futures::Stream;
use futures_util::StreamExt;
use reth_config::BodiesConfig;
use reth_consensus::Consensus;
use reth_network_p2p::{
    bodies::{
        client::BodiesClient,
        downloader::{BodyDownloader, BodyDownloaderResult},
        response::BlockResponse,
    },
    error::{DownloadError, DownloadResult},
};
use reth_primitives::SealedHeader;
use reth_storage_api::HeaderProvider;
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    mem,
    ops::RangeInclusive,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::info;

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[must_use = "Stream does nothing unless polled"]
#[derive(Debug)]
pub struct BodiesDownloader<B: BodiesClient, Provider> {
    /// The bodies client
    client: Arc<B>,
    /// The consensus client
    consensus: Arc<dyn Consensus>,
    /// The database handle
    provider: Provider,
    /// The maximum number of non-empty blocks per one request
    request_limit: u64,
    /// The maximum number of block bodies returned at once from the stream
    stream_batch_size: usize,
    /// The allowed range for number of concurrent requests.
    concurrent_requests_range: RangeInclusive<usize>,
    /// Maximum number of bytes of received blocks to buffer internally.
    max_buffered_blocks_size_bytes: usize,
    /// Current estimated size of buffered blocks in bytes.
    buffered_blocks_size_bytes: usize,
    /// The range of block numbers for body download.
    download_range: RangeInclusive<BlockNumber>,
    /// The latest block number returned.
    latest_queued_block_number: Option<BlockNumber>,
    /// Requests in progress
    in_progress_queue: BodiesRequestQueue<B>,
    /// Buffered responses
    buffered_responses: BinaryHeap<OrderedBodiesResponse>,
    /// Queued body responses that can be returned for insertion into the database.
    queued_bodies: Vec<BlockResponse>,
    /// The bodies downloader metrics.
    metrics: BodyDownloaderMetrics,
}

impl<B, Provider> BodiesDownloader<B, Provider>
where
    B: BodiesClient + 'static,
    Provider: HeaderProvider + Unpin + 'static,
{
    /// Returns the next contiguous request.
    fn next_headers_request(&self) -> DownloadResult<Option<Vec<SealedHeader>>> {
        let start_at = match self.in_progress_queue.last_requested_block_number {
            Some(num) => num + 1,
            None => *self.download_range.start(),
        };
        // as the range is inclusive, we need to add 1 to the end.
        let items_left = (self.download_range.end() + 1).saturating_sub(start_at);
        let limit = items_left.min(self.request_limit);
        self.query_headers(start_at..=*self.download_range.end(), limit)
    }

    /// Retrieve a batch of headers from the database starting from the provided block number.
    ///
    /// This method is going to return the batch as soon as one of the conditions below
    /// is fulfilled:
    ///     1. The number of non-empty headers in the batch equals requested.
    ///     2. The total number of headers in the batch (both empty and non-empty) is greater than
    ///        or equal to the stream batch size.
    ///     3. Downloader reached the end of the range
    ///
    /// NOTE: The batches returned have a variable length.
    fn query_headers(
        &self,
        range: RangeInclusive<BlockNumber>,
        max_non_empty: u64,
    ) -> DownloadResult<Option<Vec<SealedHeader>>> {
        if range.is_empty() || max_non_empty == 0 {
            return Ok(None)
        }

        // Collect headers while
        //      1. Current block number is in range
        //      2. The number of non empty headers is less than maximum
        //      3. The total number of headers is less than the stream batch size (this is only
        //         relevant if the range consists entirely of empty headers)
        let mut collected = 0;
        let mut non_empty_headers = 0;
        let headers = self.provider.sealed_headers_while(range.clone(), |header| {
            let should_take = range.contains(&header.number) &&
                non_empty_headers < max_non_empty &&
                collected < self.stream_batch_size;

            if should_take {
                collected += 1;
                if !header.is_empty() {
                    non_empty_headers += 1;
                }
                true
            } else {
                false
            }
        })?;

        Ok(Some(headers).filter(|h| !h.is_empty()))
    }

    /// Get the next expected block number for queueing.
    const fn next_expected_block_number(&self) -> BlockNumber {
        match self.latest_queued_block_number {
            Some(num) => num + 1,
            None => *self.download_range.start(),
        }
    }

    /// Max requests to handle at the same time
    ///
    /// This depends on the number of active peers but will always be
    /// [`min_concurrent_requests`..`max_concurrent_requests`]
    #[inline]
    fn concurrent_request_limit(&self) -> usize {
        let num_peers = self.client.num_connected_peers();

        let max_requests = num_peers.max(*self.concurrent_requests_range.start());

        // if we're only connected to a few peers, we keep it low
        if num_peers < *self.concurrent_requests_range.start() {
            return max_requests
        }

        max_requests.min(*self.concurrent_requests_range.end())
    }

    /// Returns true if the size of buffered blocks is lower than the configured maximum
    const fn has_buffer_capacity(&self) -> bool {
        self.buffered_blocks_size_bytes < self.max_buffered_blocks_size_bytes
    }

    // Check if the stream is terminated
    fn is_terminated(&self) -> bool {
        // There is nothing to request if the range is empty
        let nothing_to_request = self.download_range.is_empty() ||
            // or all blocks have already been requested.
            self.in_progress_queue
                .last_requested_block_number.is_some_and(|last| last == *self.download_range.end());

        nothing_to_request &&
            self.in_progress_queue.is_empty() &&
            self.buffered_responses.is_empty() &&
            self.queued_bodies.is_empty()
    }

    /// Clear all download related data.
    ///
    /// Should be invoked upon encountering fatal error.
    fn clear(&mut self) {
        self.download_range = RangeInclusive::new(1, 0);
        self.latest_queued_block_number.take();
        self.in_progress_queue.clear();
        self.queued_bodies = Vec::new();
        self.buffered_responses = BinaryHeap::new();
        self.buffered_blocks_size_bytes = 0;

        // reset metrics
        self.metrics.in_flight_requests.set(0.);
        self.metrics.buffered_responses.set(0.);
        self.metrics.buffered_blocks.set(0.);
        self.metrics.buffered_blocks_size_bytes.set(0.);
        self.metrics.queued_blocks.set(0.);
    }

    /// Queues bodies and sets the latest queued block number
    fn queue_bodies(&mut self, bodies: Vec<BlockResponse>) {
        self.latest_queued_block_number = Some(bodies.last().expect("is not empty").block_number());
        self.queued_bodies.extend(bodies);
        self.metrics.queued_blocks.set(self.queued_bodies.len() as f64);
    }

    /// Removes the next response from the buffer.
    fn pop_buffered_response(&mut self) -> Option<OrderedBodiesResponse> {
        let resp = self.buffered_responses.pop()?;
        self.metrics.buffered_responses.decrement(1.);
        self.buffered_blocks_size_bytes -= resp.size();
        self.metrics.buffered_blocks.decrement(resp.len() as f64);
        self.metrics.buffered_blocks_size_bytes.set(self.buffered_blocks_size_bytes as f64);
        Some(resp)
    }

    /// Adds a new response to the internal buffer
    fn buffer_bodies_response(&mut self, response: Vec<BlockResponse>) {
        // take into account capacity
        let size = response.iter().map(BlockResponse::size).sum::<usize>() +
            response.capacity() * mem::size_of::<BlockResponse>();

        let response = OrderedBodiesResponse { resp: response, size };
        let response_len = response.len();

        self.buffered_blocks_size_bytes += size;
        self.buffered_responses.push(response);

        self.metrics.buffered_blocks.increment(response_len as f64);
        self.metrics.buffered_blocks_size_bytes.set(self.buffered_blocks_size_bytes as f64);
        self.metrics.buffered_responses.set(self.buffered_responses.len() as f64);
    }

    /// Returns a response if it's first block number matches the next expected.
    fn try_next_buffered(&mut self) -> Option<Vec<BlockResponse>> {
        if let Some(next) = self.buffered_responses.peek() {
            let expected = self.next_expected_block_number();
            let next_block_range = next.block_range();

            if next_block_range.contains(&expected) {
                return self.pop_buffered_response().map(|buffered| {
                    buffered
                        .resp
                        .into_iter()
                        .skip_while(|b| b.block_number() < expected)
                        .take_while(|b| self.download_range.contains(&b.block_number()))
                        .collect()
                })
            }

            // Drop buffered response since we passed that range
            if *next_block_range.end() < expected {
                self.pop_buffered_response();
            }
        }
        None
    }

    /// Returns the next batch of block bodies that can be returned if we have enough buffered
    /// bodies
    fn try_split_next_batch(&mut self) -> Option<Vec<BlockResponse>> {
        if self.queued_bodies.len() >= self.stream_batch_size {
            let next_batch = self.queued_bodies.drain(..self.stream_batch_size).collect::<Vec<_>>();
            self.queued_bodies.shrink_to_fit();
            self.metrics.total_flushed.increment(next_batch.len() as u64);
            self.metrics.queued_blocks.set(self.queued_bodies.len() as f64);
            return Some(next_batch)
        }
        None
    }

    /// Check if a new request can be submitted, it implements back pressure to prevent overwhelming
    /// the system and causing memory overload.
    ///
    /// Returns true if a new request can be submitted
    fn can_submit_new_request(&self) -> bool {
        // requests are issued in order but not necessarily finished in order, so the queued bodies
        // can grow large if a certain request is slow, so we limit the followup requests if the
        // queued bodies grew too large
        self.queued_bodies.len() < 4 * self.stream_batch_size &&
            self.has_buffer_capacity() &&
            self.in_progress_queue.len() < self.concurrent_request_limit()
    }
}

impl<B, Provider> BodiesDownloader<B, Provider>
where
    B: BodiesClient + 'static,
    Provider: HeaderProvider + Unpin + 'static,
    Self: BodyDownloader + 'static,
{
    /// Spawns the downloader task via [`tokio::task::spawn`]
    pub fn into_task(self) -> TaskDownloader {
        self.into_task_with(&TokioTaskExecutor::default())
    }

    /// Convert the downloader into a [`TaskDownloader`] by spawning it via the given spawner.
    pub fn into_task_with<S>(self, spawner: &S) -> TaskDownloader
    where
        S: TaskSpawner,
    {
        TaskDownloader::spawn_with(self, spawner)
    }
}

impl<B, Provider> BodyDownloader for BodiesDownloader<B, Provider>
where
    B: BodiesClient + 'static,
    Provider: HeaderProvider + Unpin + 'static,
{
    /// Set a new download range (exclusive).
    ///
    /// This method will drain all queued bodies, filter out ones outside the range and put them
    /// back into the buffer.
    /// If there are any bodies between the range start and last queued body that have not been
    /// downloaded or are not in progress, they will be re-requested.
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()> {
        // Check if the range is valid.
        if range.is_empty() {
            tracing::error!(target: "downloaders::bodies", ?range, "Bodies download range is invalid (empty)");
            return Err(DownloadError::InvalidBodyRange { range })
        }

        // Check if the provided range is the subset of the existing range.
        let is_current_range_subset = self.download_range.contains(range.start()) &&
            *range.end() == *self.download_range.end();
        if is_current_range_subset {
            tracing::trace!(target: "downloaders::bodies", ?range, "Download range already in progress");
            // The current range already includes requested.
            return Ok(())
        }

        // Check if the provided range is the next expected range.
        let count = *range.end() - *range.start() + 1; // range is inclusive
        let is_next_consecutive_range = *range.start() == *self.download_range.end() + 1;
        if is_next_consecutive_range {
            // New range received.
            tracing::trace!(target: "downloaders::bodies", ?range, "New download range set");
            info!(target: "downloaders::bodies", count, ?range, "Downloading bodies");
            self.download_range = range;
            return Ok(())
        }

        // The block range is reset. This can happen either after unwind or after the bodies were
        // written by external services (e.g. BlockchainTree).
        tracing::trace!(target: "downloaders::bodies", ?range, prev_range = ?self.download_range, "Download range reset");
        info!(target: "downloaders::bodies", count, ?range, "Downloading bodies");
        self.clear();
        self.download_range = range;
        Ok(())
    }
}

impl<B, Provider> Stream for BodiesDownloader<B, Provider>
where
    B: BodiesClient + 'static,
    Provider: HeaderProvider + Unpin + 'static,
{
    type Item = BodyDownloaderResult;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.is_terminated() {
            return Poll::Ready(None)
        }
        // Submit new requests and poll any in progress
        loop {
            // Yield next batch if ready
            if let Some(next_batch) = this.try_split_next_batch() {
                return Poll::Ready(Some(Ok(next_batch)))
            }

            // Poll requests
            while let Poll::Ready(Some(response)) = this.in_progress_queue.poll_next_unpin(cx) {
                this.metrics.in_flight_requests.decrement(1.);
                match response {
                    Ok(response) => {
                        this.buffer_bodies_response(response);
                    }
                    Err(error) => {
                        tracing::debug!(target: "downloaders::bodies", %error, "Request failed");
                        this.clear();
                        return Poll::Ready(Some(Err(error)))
                    }
                };
            }

            // Loop exit condition
            let mut new_request_submitted = false;
            // Submit new requests
            'inner: while this.can_submit_new_request() {
                match this.next_headers_request() {
                    Ok(Some(request)) => {
                        this.metrics.in_flight_requests.increment(1.);
                        this.in_progress_queue.push_new_request(
                            Arc::clone(&this.client),
                            Arc::clone(&this.consensus),
                            request,
                        );
                        new_request_submitted = true;
                    }
                    Ok(None) => break 'inner,
                    Err(error) => {
                        tracing::error!(target: "downloaders::bodies", %error, "Failed to download from next request");
                        this.clear();
                        return Poll::Ready(Some(Err(error)))
                    }
                };
            }

            while let Some(buf_response) = this.try_next_buffered() {
                this.queue_bodies(buf_response);
            }

            // shrink the buffer so that it doesn't grow indefinitely
            this.buffered_responses.shrink_to_fit();

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
            let next_batch = this.queued_bodies.drain(..batch_size).collect::<Vec<_>>();
            this.queued_bodies.shrink_to_fit();
            this.metrics.total_flushed.increment(next_batch.len() as u64);
            this.metrics.queued_blocks.set(this.queued_bodies.len() as f64);
            return Poll::Ready(Some(Ok(next_batch)))
        }

        Poll::Pending
    }
}

#[derive(Debug)]
struct OrderedBodiesResponse {
    resp: Vec<BlockResponse>,
    /// The total size of the response in bytes
    size: usize,
}

impl OrderedBodiesResponse {
    /// Returns the block number of the first element
    ///
    /// # Panics
    /// If the response vec is empty.
    fn first_block_number(&self) -> u64 {
        self.resp.first().expect("is not empty").block_number()
    }

    /// Returns the range of the block numbers in the response
    ///
    /// # Panics
    /// If the response vec is empty.
    fn block_range(&self) -> RangeInclusive<u64> {
        self.first_block_number()..=self.resp.last().expect("is not empty").block_number()
    }

    #[inline]
    fn len(&self) -> usize {
        self.resp.len()
    }

    /// Returns the size of the response in bytes
    ///
    /// See [`BlockResponse::size`]
    #[inline]
    const fn size(&self) -> usize {
        self.size
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

/// Builder for [`BodiesDownloader`].
#[derive(Debug, Clone)]
pub struct BodiesDownloaderBuilder {
    /// The batch size of non-empty blocks per one request
    pub request_limit: u64,
    /// The maximum number of block bodies returned at once from the stream
    pub stream_batch_size: usize,
    /// Maximum number of bytes of received bodies to buffer internally.
    pub max_buffered_blocks_size_bytes: usize,
    /// The maximum number of requests to send concurrently.
    pub concurrent_requests_range: RangeInclusive<usize>,
}

impl BodiesDownloaderBuilder {
    /// Creates a new [`BodiesDownloaderBuilder`] with configurations based on the provided
    /// [`BodiesConfig`].
    pub fn new(config: BodiesConfig) -> Self {
        Self::default()
            .with_stream_batch_size(config.downloader_stream_batch_size)
            .with_request_limit(config.downloader_request_limit)
            .with_max_buffered_blocks_size_bytes(config.downloader_max_buffered_blocks_size_bytes)
            .with_concurrent_requests_range(
                config.downloader_min_concurrent_requests..=
                    config.downloader_max_concurrent_requests,
            )
    }
}

impl Default for BodiesDownloaderBuilder {
    fn default() -> Self {
        Self {
            request_limit: 200,
            stream_batch_size: 1_000,
            max_buffered_blocks_size_bytes: 2 * 1024 * 1024 * 1024, // ~2GB
            concurrent_requests_range: 5..=100,
        }
    }
}

impl BodiesDownloaderBuilder {
    /// Set request batch size on the downloader.
    pub const fn with_request_limit(mut self, request_limit: u64) -> Self {
        self.request_limit = request_limit;
        self
    }

    /// Set stream batch size on the downloader.
    pub const fn with_stream_batch_size(mut self, stream_batch_size: usize) -> Self {
        self.stream_batch_size = stream_batch_size;
        self
    }

    /// Set concurrent requests range on the downloader.
    pub const fn with_concurrent_requests_range(
        mut self,
        concurrent_requests_range: RangeInclusive<usize>,
    ) -> Self {
        self.concurrent_requests_range = concurrent_requests_range;
        self
    }

    /// Set max buffered block bytes on the downloader.
    pub const fn with_max_buffered_blocks_size_bytes(
        mut self,
        max_buffered_blocks_size_bytes: usize,
    ) -> Self {
        self.max_buffered_blocks_size_bytes = max_buffered_blocks_size_bytes;
        self
    }

    /// Consume self and return the concurrent downloader.
    pub fn build<B, Provider>(
        self,
        client: B,
        consensus: Arc<dyn Consensus>,
        provider: Provider,
    ) -> BodiesDownloader<B, Provider>
    where
        B: BodiesClient + 'static,
        Provider: HeaderProvider,
    {
        let Self {
            request_limit,
            stream_batch_size,
            concurrent_requests_range,
            max_buffered_blocks_size_bytes,
        } = self;
        let metrics = BodyDownloaderMetrics::default();
        let in_progress_queue = BodiesRequestQueue::new(metrics.clone());
        BodiesDownloader {
            client: Arc::new(client),
            consensus,
            provider,
            request_limit,
            stream_batch_size,
            max_buffered_blocks_size_bytes,
            concurrent_requests_range,
            in_progress_queue,
            metrics,
            download_range: RangeInclusive::new(1, 0),
            latest_queued_block_number: None,
            buffered_responses: Default::default(),
            queued_bodies: Default::default(),
            buffered_blocks_size_bytes: 0,
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
    use alloy_primitives::B256;
    use assert_matches::assert_matches;
    use reth_chainspec::MAINNET;
    use reth_consensus::test_utils::TestConsensus;
    use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
    use reth_provider::{
        providers::StaticFileProvider, test_utils::MockNodeTypesWithDB, ProviderFactory,
    };
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};
    use std::collections::HashMap;

    // Check that the blocks are emitted in order of block number, not in order of
    // first-downloaded
    #[tokio::test]
    async fn streams_bodies_in_order() {
        // Generate some random blocks
        let db = create_test_rw_db();
        let (headers, mut bodies) = generate_bodies(0..=19);

        insert_headers(db.db(), &headers);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let (_static_dir, static_dir_path) = create_test_static_files_dir();

        let mut downloader = BodiesDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            ProviderFactory::<MockNodeTypesWithDB>::new(
                db,
                MAINNET.clone(),
                StaticFileProvider::read_write(static_dir_path).unwrap(),
            ),
        );
        downloader.set_download_range(0..=19).expect("failed to set download range");

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
        let db = create_test_rw_db();
        let mut rng = generators::rng();
        let blocks = random_block_range(
            &mut rng,
            0..=199,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 1..2, ..Default::default() },
        );

        let headers = blocks.iter().map(|block| block.header.clone()).collect::<Vec<_>>();
        let bodies =
            blocks.into_iter().map(|block| (block.hash(), block.body)).collect::<HashMap<_, _>>();

        insert_headers(db.db(), &headers);

        let request_limit = 10;
        let client = Arc::new(TestBodiesClient::default().with_bodies(bodies.clone()));
        let (_static_dir, static_dir_path) = create_test_static_files_dir();

        let mut downloader =
            BodiesDownloaderBuilder::default().with_request_limit(request_limit).build(
                client.clone(),
                Arc::new(TestConsensus::default()),
                ProviderFactory::<MockNodeTypesWithDB>::new(
                    db,
                    MAINNET.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
            );
        downloader.set_download_range(0..=199).expect("failed to set download range");

        let _ = downloader.collect::<Vec<_>>().await;
        assert_eq!(client.times_requested(), 20);
    }

    // Check that bodies are returned in correct order
    // after resetting the download range multiple times.
    #[tokio::test]
    async fn streams_bodies_in_order_after_range_reset() {
        // Generate some random blocks
        let db = create_test_rw_db();
        let (headers, mut bodies) = generate_bodies(0..=99);

        insert_headers(db.db(), &headers);

        let stream_batch_size = 20;
        let request_limit = 10;
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let mut downloader = BodiesDownloaderBuilder::default()
            .with_stream_batch_size(stream_batch_size)
            .with_request_limit(request_limit)
            .build(
                client.clone(),
                Arc::new(TestConsensus::default()),
                ProviderFactory::<MockNodeTypesWithDB>::new(
                    db,
                    MAINNET.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
            );

        let mut range_start = 0;
        while range_start < 100 {
            downloader.set_download_range(range_start..=99).expect("failed to set download range");

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
        let db = create_test_rw_db();
        let (headers, mut bodies) = generate_bodies(0..=199);

        insert_headers(db.db(), &headers);

        let client = Arc::new(TestBodiesClient::default().with_bodies(bodies.clone()));
        let (_static_dir, static_dir_path) = create_test_static_files_dir();

        let mut downloader = BodiesDownloaderBuilder::default().with_stream_batch_size(100).build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            ProviderFactory::<MockNodeTypesWithDB>::new(
                db,
                MAINNET.clone(),
                StaticFileProvider::read_write(static_dir_path).unwrap(),
            ),
        );

        // Set and download the first range
        downloader.set_download_range(0..=99).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().take(100), &mut bodies))
        );

        // Check that the stream is terminated
        assert!(downloader.next().await.is_none());

        // Set and download the second range
        downloader.set_download_range(100..=199).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().skip(100), &mut bodies))
        );
    }

    // Check that the downloader continues after the size limit is reached.
    #[tokio::test]
    async fn can_download_after_exceeding_limit() {
        // Generate some random blocks
        let db = create_test_rw_db();
        let (headers, mut bodies) = generate_bodies(0..=199);

        insert_headers(db.db(), &headers);

        let client = Arc::new(TestBodiesClient::default().with_bodies(bodies.clone()));

        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        // Set the max buffered block size to 1 byte, to make sure that every response exceeds the
        // limit
        let mut downloader = BodiesDownloaderBuilder::default()
            .with_stream_batch_size(10)
            .with_request_limit(1)
            .with_max_buffered_blocks_size_bytes(1)
            .build(
                client.clone(),
                Arc::new(TestConsensus::default()),
                ProviderFactory::<MockNodeTypesWithDB>::new(
                    db,
                    MAINNET.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
            );

        // Set and download the entire range
        downloader.set_download_range(0..=199).expect("failed to set download range");
        let mut header = 0;
        while let Some(Ok(resp)) = downloader.next().await {
            assert_eq!(resp, zip_blocks(headers.iter().skip(header).take(resp.len()), &mut bodies));
            header += resp.len();
        }
    }

    // Check that the downloader can tolerate a few completely empty responses
    #[tokio::test]
    async fn can_tolerate_empty_responses() {
        // Generate some random blocks
        let db = create_test_rw_db();
        let (headers, mut bodies) = generate_bodies(0..=99);

        insert_headers(db.db(), &headers);

        // respond with empty bodies for every other request.
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_empty_responses(2),
        );
        let (_static_dir, static_dir_path) = create_test_static_files_dir();

        let mut downloader = BodiesDownloaderBuilder::default()
            .with_request_limit(3)
            .with_stream_batch_size(100)
            .build(
                client.clone(),
                Arc::new(TestConsensus::default()),
                ProviderFactory::<MockNodeTypesWithDB>::new(
                    db,
                    MAINNET.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
            );

        // Download the requested range
        downloader.set_download_range(0..=99).expect("failed to set download range");
        assert_matches!(
            downloader.next().await,
            Some(Ok(res)) => assert_eq!(res, zip_blocks(headers.iter().take(100), &mut bodies))
        );
    }
}
