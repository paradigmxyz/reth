use super::queue::BodiesRequestQueue;
use futures::Stream;
use futures_util::StreamExt;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{
    consensus::Consensus as ConsensusTrait,
    db,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader, response::BlockResponse},
        downloader::Downloader,
    },
};
use reth_primitives::{BlockNumber, SealedHeader};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    collections::BinaryHeap,
    ops::{Range, RangeInclusive},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[derive(Debug)]
pub struct ConcurrentDownloader<B, C, DB> {
    /// The bodies client
    client: Arc<B>,
    /// The consensus client
    consensus: Arc<C>,
    /// The database handle
    db: Arc<DB>,
    /// The batch size of non-empty blocks per one request
    request_batch_size: usize,
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
    in_progress_queue: BodiesRequestQueue<B, C>,
    /// Buffered responses
    buffered_responses: BinaryHeap<OrderedBodiesResponse>,
    /// Queued body responses
    queued_bodies: Vec<BlockResponse>,
}

impl<B, C, DB> ConcurrentDownloader<B, C, DB>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
    DB: Database,
{
    fn new(
        client: Arc<B>,
        consensus: Arc<C>,
        db: Arc<DB>,
        request_batch_size: usize,
        stream_batch_size: usize,
        max_buffered_responses: usize,
        concurrent_requests_range: RangeInclusive<usize>,
    ) -> Self {
        Self {
            client,
            consensus,
            db,
            request_batch_size,
            stream_batch_size,
            max_buffered_responses,
            concurrent_requests_range,
            download_range: Default::default(),
            latest_queued_block_number: None,
            in_progress_queue: Default::default(),
            buffered_responses: Default::default(),
            queued_bodies: Default::default(),
        }
    }

    /// Returns the next contiguous request.
    fn next_headers_request(&mut self) -> Result<Option<Vec<SealedHeader>>, db::Error> {
        let start_at = match self.in_progress_queue.last_requested_block_number {
            Some(num) => num + 1,
            None => self.download_range.start,
        };

        let request = self.query_headers(start_at, self.request_batch_size)?;

        if request.is_empty() {
            return Ok(None)
        }

        Ok(Some(request))
    }

    fn query_headers(
        &self,
        start: BlockNumber,
        count: usize,
    ) -> Result<Vec<SealedHeader>, db::Error> {
        let tx = self.db.tx()?;

        // Acquire cursors over canonical and header tables
        let mut canonical_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;

        // TODO: experimental
        let mut non_empty_headers = 0;
        let mut headers = Vec::<SealedHeader>::default();
        let mut canonical_entry = canonical_cursor.seek_exact(start)?;
        while let Some((number, hash)) = canonical_entry {
            let key = (number, hash).into();
            let (_, header) = header_cursor.seek_exact(key)?.expect("database corrupted");
            if !header.is_empty() {
                non_empty_headers += 1;
            }
            headers.push(SealedHeader::new(header, key.hash()));
            if non_empty_headers >= count || headers.len() >= self.stream_batch_size {
                break
            }
            canonical_entry = canonical_cursor.next()?;
        }

        return Ok(headers)
    }

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
        let dynamic_target = num_peers * 4;
        let max_dynamic = dynamic_target.max(*self.concurrent_requests_range.start());

        // If only a few peers are connected we keep it low
        if num_peers < *self.concurrent_requests_range.start() {
            return max_dynamic
        }

        max_dynamic.min(*self.concurrent_requests_range.end())
    }

    fn is_terminated(&self) -> bool {
        self.in_progress_queue
            .last_requested_block_number
            .map(|last| last + 1 == self.download_range.end)
            .unwrap_or_default() &&
            self.in_progress_queue.is_empty() &&
            self.buffered_responses.is_empty() &&
            self.queued_bodies.is_empty()
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
                self.buffered_responses.pop();
            }
        }
        None
    }
}

impl<B, C, DB> Downloader for ConcurrentDownloader<B, C, DB>
where
    B: BodiesClient,
    C: ConsensusTrait,
    DB: Database,
{
    type Client = B;
    type Consensus = C;

    fn client(&self) -> &Self::Client {
        self.client.borrow()
    }

    fn consensus(&self) -> &Self::Consensus {
        self.consensus.borrow()
    }
}

impl<B, C, DB> BodyDownloader for ConcurrentDownloader<B, C, DB>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
    DB: Database,
{
    fn set_download_range(&mut self, range: Range<BlockNumber>) -> Result<(), db::Error> {
        if range.is_empty() {
            tracing::warn!(target: "downloaders::bodies", "New header range is empty");
            return Ok(())
        }

        tracing::trace!(target: "downloaders::bodies", ?range, "Setting new header range");

        // Drain queued bodies.
        let queued_bodies = std::mem::take(&mut self.queued_bodies)
            .into_iter()
            .filter(|b| range.contains(&b.block_number()))
            .collect::<Vec<_>>();
        tracing::trace!(target: "downloaders::bodies", len = queued_bodies.len(), "Evicted bodies from the queue");

        // Dispatch requests for missing bodies
        if let Some(latest_queued) = self.latest_queued_block_number {
            if range.start <= latest_queued {
                let queued_bodies_range = match (queued_bodies.first(), queued_bodies.last()) {
                    (Some(first), Some(last)) => first.block_number()..last.block_number() + 1,
                    _ => Range::default(),
                };
                let mut request_range = Range::default();
                for num in range.start..=latest_queued {
                    // Check if block is already in the buffer or in progress
                    if queued_bodies_range.contains(&num) ||
                        self.in_progress_queue.contains_block(num)
                    {
                        continue
                    }

                    if range.is_empty() {
                        request_range.start = num;
                    } else if request_range.end + 1 == num {
                        request_range.end = num;
                    } else {
                        // Dispatch contiguous request.
                        self.in_progress_queue.push_new_request(
                            Arc::clone(&self.client),
                            Arc::clone(&self.consensus),
                            self.query_headers(request_range.start, request_range.count())?,
                        );
                        // Clear the current request range
                        request_range = Range::default();
                    }
                }
            }
        }

        // Put queued bodies into the buffer
        if !queued_bodies.is_empty() {
            self.buffered_responses.push(OrderedBodiesResponse(queued_bodies));
        }

        self.download_range = range;
        self.latest_queued_block_number = None;
        tracing::trace!(target: "downloaders::bodies", range = ?self.download_range, "New header range set");
        Ok(())
    }
}

impl<B, C, DB> Stream for ConcurrentDownloader<B, C, DB>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
    DB: Database,
{
    type Item = Vec<BlockResponse>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.is_terminated() {
            return Poll::Ready(None)
        }

        // Yield next batch
        if this.queued_bodies.len() > this.stream_batch_size {
            let next_batch = this.queued_bodies.drain(..this.stream_batch_size);
            return Poll::Ready(Some(next_batch.collect()))
        }

        // Submit new requests and poll any in progress
        loop {
            // Poll requests
            if let Poll::Ready(Some(response)) = this.in_progress_queue.poll_next_unpin(cx) {
                let response = OrderedBodiesResponse(response);
                this.buffered_responses.push(response);
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
                        this.in_progress_queue.push_new_request(
                            Arc::clone(&this.client),
                            Arc::clone(&this.consensus),
                            request,
                        );
                        new_request_submitted = true;
                    }
                    Ok(None) => break 'inner,
                    Err(error) => {
                        tracing::error!(target: "downloaders::bodies", ?error, "Failed to form next request");
                    }
                };
            }

            while let Some(buf_response) = this.try_next_buffered() {
                this.queue_bodies(buf_response);
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
            let resp = next_batch.collect::<Vec<_>>();
            return Poll::Ready(Some(resp))
        }

        Poll::Pending
    }
}

/// SAFETY: we need to ensure `ConcurrentDownloader` is `Sync` because the of the [Downloader]
/// trait. While [HeadersClient] is also `Sync`, the [HeadersClient::get_block_bodies] future does
/// not enforce `Sync` (async_trait). The future itself does not use any interior mutability
/// whatsoever: All the mutations are performed through an exclusive reference on
/// `ConcurrentDownloader` when the Stream is polled. This means it suffices that
/// `ConcurrentDownloader` is Sync:
unsafe impl<B, C, DB> Sync for ConcurrentDownloader<B, C, DB>
where
    B: BodiesClient,
    C: ConsensusTrait,
    DB: Database,
{
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

/// Builder for [ConcurrentDownloader].
pub struct ConcurrentDownloaderBuilder {
    /// The batch size of non-empty blocks per one request
    request_batch_size: usize,
    /// The maximum number of block bodies returned at once from the stream
    stream_batch_size: usize,
    /// Maximum amount of received bodies to buffer internally.
    max_buffered_responses: usize,
    /// The maximum number of requests to send concurrently.
    concurrent_requests_range: RangeInclusive<usize>,
}

impl Default for ConcurrentDownloaderBuilder {
    fn default() -> Self {
        Self {
            request_batch_size: 100,
            stream_batch_size: 1000,
            max_buffered_responses: 10000,
            concurrent_requests_range: 5..=100,
        }
    }
}

impl ConcurrentDownloaderBuilder {
    /// Set request batch size on the downloader.
    pub fn with_request_batch_size(mut self, request_batch_size: usize) -> Self {
        self.request_batch_size = request_batch_size;
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
    pub fn build<B, C, DB>(
        self,
        client: Arc<B>,
        consensus: Arc<C>,
        db: Arc<DB>,
    ) -> ConcurrentDownloader<B, C, DB>
    where
        C: ConsensusTrait + 'static,
        B: BodiesClient + 'static,
        DB: Database,
    {
        ConcurrentDownloader::new(
            client,
            consensus,
            db,
            self.request_batch_size,
            self.stream_batch_size,
            self.max_buffered_responses,
            self.concurrent_requests_range,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::test_utils::zip_blocks,
        test_utils::{generate_bodies, TestBodiesClient},
    };
    use assert_matches::assert_matches;
    use futures_util::stream::StreamExt;
    use reth_db::{
        mdbx::{test_utils::create_test_db, Env, EnvKind, WriteMap},
        transaction::DbTxMut,
    };
    use reth_interfaces::test_utils::TestConsensus;
    use std::sync::Arc;

    #[inline]
    fn insert_headers(db: &Env<WriteMap>, headers: &[SealedHeader]) {
        db.update(|tx| -> Result<(), db::Error> {
            for header in headers {
                tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
                tx.put::<tables::Headers>(header.num_hash().into(), header.clone().unseal())?;
            }
            Ok(())
        })
        .expect("failed to commit")
        .expect("failed to insert headers");
    }

    // Check that the blocks are emitted in order of block number, not in order of
    // first-downloaded
    #[tokio::test]
    async fn streams_bodies_in_order() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..20);

        insert_headers(db.borrow(), &headers);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let mut downloader = ConcurrentDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );
        downloader.set_download_range(0..20).expect("failed to set header range");

        assert_matches!(
            downloader.next().await,
            Some(res) => assert_eq!(res, zip_blocks(headers.iter(),&mut bodies))
        );
        assert_eq!(client.times_requested(), 1);
    }

    // Check TODO:
    #[tokio::test]
    async fn streams_bodies_in_order_after_range_reset() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, mut bodies) = generate_bodies(0..100);

        insert_headers(db.borrow(), &headers);

        let stream_batch_size = 20;
        let request_batch_size = 10;
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let mut downloader = ConcurrentDownloaderBuilder::default()
            .with_stream_batch_size(stream_batch_size)
            .with_request_batch_size(request_batch_size)
            .build(client.clone(), Arc::new(TestConsensus::default()), db);

        let mut range_start = 0;
        while range_start < 100 {
            downloader.set_download_range(range_start..100).expect("failed to set header range");

            assert_matches!(
                downloader.next().await,
                Some(res) => assert_eq!(res, zip_blocks(headers.iter().skip(range_start as usize).take(stream_batch_size), &mut bodies))
            );
            range_start += stream_batch_size as u64;
        }
        assert_eq!(
            client.times_requested(),
            // div_ceil
            ((headers.iter().filter(|x| !x.is_empty()).count() + 9) / 10) as u64,
        );
    }
}
