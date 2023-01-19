use futures::{stream::FuturesUnordered, Future, FutureExt, Stream};
use futures_util::StreamExt;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    consensus::{Consensus as ConsensusTrait, Error as ConsensusError},
    db,
    p2p::{
        bodies::{
            client::BodiesClient,
            downloader::BodyDownloader,
            response::{BlockResponse, OrderedBlockResponse},
        },
        downloader::Downloader,
        error::{PeerRequestResult, RequestError},
    },
};
use reth_primitives::{BlockNumber, PeerId, SealedBlock, SealedHeader, H256};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    collections::BinaryHeap,
    ops::{Range, RangeInclusive},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use thiserror::Error;

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
    /// The number of block bodies to return at once
    stream_batch_size: usize,
    /// The maximum number of requests to send concurrently.
    max_concurrent_requests: usize,
    /// Maximum amount of received headers to buffer internally.
    max_buffered_responses: usize,
    /// The range of headers for body download.
    header_range: Range<BlockNumber>,
    /// The last requested block number.
    last_requested_block_number: Option<BlockNumber>,
    /// The latest block number returned.
    latest_queued_block_number: Option<BlockNumber>,
    /// Requests in progress
    in_progress_queue: FuturesUnordered<BodiesRequestFuture<B, C>>,
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
        max_concurrent_requests: usize,
        max_buffered_responses: usize,
    ) -> Self {
        Self {
            client,
            consensus,
            db,
            request_batch_size,
            stream_batch_size,
            max_concurrent_requests,
            max_buffered_responses,
            header_range: Default::default(),
            last_requested_block_number: None,
            latest_queued_block_number: None,
            in_progress_queue: Default::default(),
            buffered_responses: Default::default(),
            queued_bodies: Default::default(),
        }
    }

    /// Returns the next contiguous request.
    fn next_request_fut(&mut self) -> Result<Option<BodiesRequestFuture<B, C>>, db::Error> {
        let start_at = match self.last_requested_block_number {
            Some(num) => num + 1,
            None => self.header_range.start,
        };

        let request = self.query_headers(start_at, self.request_batch_size)?;

        if request.is_empty() {
            return Ok(None)
        }

        self.last_requested_block_number = request.last().map(|h| h.number);
        Ok(Some(
            BodiesRequestFuture::new(Arc::clone(&self.client), Arc::clone(&self.consensus))
                .with_headers(request),
        ))
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
            if non_empty_headers >= count || non_empty_headers > self.stream_batch_size {
                break
            }
            canonical_entry = canonical_cursor.next()?;
        }

        return Ok(headers)
    }

    fn next_expected_block_number(&self) -> BlockNumber {
        match self.latest_queued_block_number {
            Some(num) => num + 1,
            None => self.header_range.start,
        }
    }

    fn has_capacity(&self) -> bool {
        self.in_progress_queue.len() < self.max_concurrent_requests &&
            self.buffered_responses.len() < self.max_buffered_responses
    }

    fn is_terminated(&self) -> bool {
        self.last_requested_block_number
            .map(|last| last + 1 == self.header_range.end)
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
                    buffered.0.into_iter().skip_while(|b| b.block_number() < expected).collect()
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
    fn set_header_range(&mut self, range: Range<BlockNumber>) -> Result<(), db::Error> {
        if range.is_empty() {
            tracing::warn!(target: "downloaders::bodies", "New header range is empty");
            return Ok(())
        }

        // Collect buffered bodies from queue and buffer
        let mut buffered_bodies = BinaryHeap::<OrderedBlockResponse>::default();

        // Drain currently buffered responses. Retain requested bodies.
        for buffered in self.buffered_responses.drain() {
            for body in buffered.0 {
                if range.contains(&body.block_number()) {
                    buffered_bodies.push(body.into());
                }
            }
        }

        // Drain queued bodies.
        for queued in self.queued_bodies.drain(..) {
            if range.contains(&queued.block_number()) {
                buffered_bodies.push(queued.into());
            }
        }

        let bodies = buffered_bodies.into_sorted_vec(); // ascending

        // Dispatch requests for missing bodies
        if let Some(last_requested) = self.last_requested_block_number {
            let mut request_range = Range::default();
            for num in range.start..=last_requested {
                // Check if block is already in the buffer
                if bodies.iter().any(|b| num == b.block_number()) {
                    continue
                }

                // Check if block is already in progress
                if self
                    .in_progress_queue
                    .iter()
                    .any(|fut| fut.headers.iter().any(|h| h.number == num))
                {
                    continue
                }

                if range.is_empty() {
                    request_range.start = num;
                } else if request_range.end + 1 == num {
                    request_range.end = num;
                } else {
                    // Dispatch contiguous request.
                    self.in_progress_queue.push(
                        BodiesRequestFuture::new(
                            Arc::clone(&self.client),
                            Arc::clone(&self.consensus),
                        )
                        .with_headers(
                            self.query_headers(request_range.start, request_range.count())?,
                        ),
                    );
                    // Clear the current request range
                    request_range = Range::default();
                }
            }
        }

        // Put buffered bodies back into the buffer.
        let mut bodies = bodies.into_iter().peekable();
        while bodies.peek().is_some() {
            let mut contigious_batch = Vec::from([bodies.next().unwrap().0]);
            while bodies
                .peek()
                .map(|b| b.block_number() == contigious_batch.last().unwrap().block_number() + 1)
                .unwrap_or_default()
            {
                contigious_batch.push(bodies.next().unwrap().0);
            }

            self.buffered_responses.push(OrderedBodiesResponse(contigious_batch));
        }

        self.header_range = range;
        self.latest_queued_block_number = None;
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
            // Submit new requests
            while this.has_capacity() {
                match this.next_request_fut() {
                    Ok(Some(fut)) => this.in_progress_queue.push(fut),
                    Ok(None) => break, // no more requests
                    Err(error) => {
                        tracing::error!(target: "downloaders::bodies", ?error, "Failed to form next request");
                    }
                };
            }

            // Poll requests
            if let Poll::Ready(Some(response)) = this.in_progress_queue.poll_next_unpin(cx) {
                let response = OrderedBodiesResponse(response);
                this.buffered_responses.push(response);
                while let Some(buf_response) = this.try_next_buffered() {
                    this.queue_bodies(buf_response);
                }
            } else {
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
            return Poll::Ready(Some(next_batch.collect()))
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

type BodiesFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<BlockBody>>> + Send>>;

#[derive(Error, Debug)]
enum BodyRequestError {
    #[error("Received empty response")]
    EmptyResponse,
    #[error("Received more bodies than requested. Expected: {expected}. Received: {received}")]
    TooManyBodies { expected: usize, received: usize },
    #[error("Error validating body for header {hash:?}: {error}")]
    Validation { hash: H256, error: ConsensusError },
    #[error(transparent)]
    Request(#[from] RequestError),
}

/// Body request implemented as a [Future].
///
/// The future will poll the underlying request until fullfilled.
/// If the response arrived with insufficient number of bodies, the future
/// will issue another request until all bodies are collected.
///
/// It then proceeds to verify the downloaded bodies. In case of an validation error,
/// the future will start over.
///
/// The future will filter out any empty headers (see [SealedHeader::is_empty]) from the request.
/// If [BodiesRequestFuture] was initialiazed with all empty headers, no request will be dispatched
/// and they will be immediately returned upon polling.
///
/// NB: This assumes that peers respond with bodies in the order that they were requested.
/// This is a reasonable assumption to make as that's [what Geth
/// does](https://github.com/ethereum/go-ethereum/blob/f53ff0ff4a68ffc56004ab1d5cc244bcb64d3277/les/server_requests.go#L245).
/// All errors regarding the response cause the peer to get penalized, meaning that adversaries
/// that try to give us bodies that do not match the requested order are going to be penalized
/// and eventually disconnected.
struct BodiesRequestFuture<B, C> {
    client: Arc<B>,
    consensus: Arc<C>,
    // All requested headers
    headers: Vec<SealedHeader>,
    // Remaining hashes to download
    hashes_to_download: Vec<H256>,
    buffer: Vec<(PeerId, BlockBody)>,
    fut: Option<BodiesFut>,
}

impl<B, C> BodiesRequestFuture<B, C>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
{
    /// Returns an empty future. Use [BodiesRequestFuture::with_headers] to set the request.
    fn new(client: Arc<B>, consensus: Arc<C>) -> Self {
        Self {
            client,
            consensus,
            headers: Default::default(),
            hashes_to_download: Default::default(),
            buffer: Default::default(),
            fut: None,
        }
    }

    fn with_headers(mut self, headers: Vec<SealedHeader>) -> Self {
        self.headers = headers;
        self.reset_hashes();
        // Submit the request only if there are any headers to download.
        // Otherwise, the future will immediately be resolved.
        if !self.hashes_to_download.is_empty() {
            self.submit_request();
        }
        self
    }

    fn on_error(&mut self, error: BodyRequestError, peer_id: Option<PeerId>) {
        tracing::error!(target: "downloaders::bodies", ?peer_id, request = ?self.hashes_to_download, %error, "Error requesting bodies");
        if let Some(peer_id) = peer_id {
            self.client.report_bad_message(peer_id);
        }
        self.submit_request();
    }

    fn submit_request(&mut self) {
        let client = Arc::clone(&self.client);
        let request = self.hashes_to_download.clone();
        tracing::trace!(target: "downloaders::bodies", ?request, request_len = request.len(), "Requesting bodies");
        self.fut = Some(Box::pin(async move { client.get_block_bodies(request).await }));
    }

    fn reset_hashes(&mut self) {
        self.hashes_to_download =
            self.headers.iter().filter(|h| !h.is_empty()).map(|h| h.hash()).collect();
    }

    /// Attempt to construct blocks from origin headers and buffered bodies.
    ///
    /// NOTE: This method drains the buffer.
    ///
    /// # Panics
    ///
    /// If the number of buffered bodies does not equal the number of non empty headers.
    fn try_construct_blocks(&mut self) -> Result<Vec<BlockResponse>, (PeerId, BodyRequestError)> {
        let mut bodies = self.buffer.drain(..);
        let mut results = Vec::default();
        for header in self.headers.iter().cloned() {
            if header.is_empty() {
                results.push(BlockResponse::Empty(header));
            } else {
                // The body must be present since we requested headers for all non-empty
                // bodies at this point
                let (peer_id, body) = bodies.next().expect("download logic failed");

                let block = SealedBlock {
                    header: header.clone(),
                    body: body.transactions,
                    ommers: body.ommers.into_iter().map(|header| header.seal()).collect(),
                };

                // This ensures that the TxRoot and OmmersRoot from the header match the
                // ones calculated manually from the block body.
                self.consensus.pre_validate_block(&block).map_err(|error| {
                    (peer_id, BodyRequestError::Validation { hash: header.hash(), error })
                })?;

                results.push(BlockResponse::Full(block));
            }
        }
        Ok(results)
    }
}

impl<B, C> Future for BodiesRequestFuture<B, C>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
{
    type Output = Vec<BlockResponse>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            if let Some(fut) = this.fut.as_mut() {
                match ready!(fut.poll_unpin(cx)) {
                    Ok(response) => {
                        let (peer_id, bodies) = response.split();
                        if bodies.is_empty() {
                            this.on_error(BodyRequestError::EmptyResponse, Some(peer_id));
                            continue
                        }

                        let request_len = this.hashes_to_download.len();
                        let response_len = bodies.len();
                        if response_len > request_len {
                            this.on_error(
                                BodyRequestError::TooManyBodies {
                                    expected: request_len,
                                    received: response_len,
                                },
                                Some(peer_id),
                            );
                            continue
                        }

                        tracing::trace!(
                            target: "downloaders::bodies", request_len, response_len, ?peer_id, "Received bodies"
                        );
                        this.hashes_to_download.drain(..response_len);
                        this.buffer.extend(bodies.into_iter().map(|b| (peer_id, b)));

                        if !this.hashes_to_download.is_empty() {
                            // Submit next request if not done
                            this.submit_request();
                        } else {
                            this.fut = None;
                        }
                    }
                    Err(error) => {
                        this.on_error(error.into(), None);
                    }
                }
            }

            // Drain the buffer and attempt to construct the response.
            // If validation fails, the future will restart from scratch.
            if this.hashes_to_download.is_empty() {
                match this.try_construct_blocks() {
                    Ok(blocks) => return Poll::Ready(blocks),
                    Err((peer_id, error)) => {
                        this.reset_hashes();
                        this.on_error(error, Some(peer_id));
                    }
                }
                // Sanity check
            } else if this.fut.is_none() {
                panic!("Body request logic failure")
            }
        }
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

/// Builder for [ConcurrentDownloader].
pub struct ConcurrentDownloaderBuilder {
    /// The batch size of non-empty blocks per one request
    request_batch_size: usize,
    /// The number of block bodies to return at once
    stream_batch_size: usize,
    /// The maximum number of requests to send concurrently.
    max_concurrent_requests: usize,
    /// Maximum amount of received headers to buffer internally.
    max_buffered_responses: usize,
}

impl Default for ConcurrentDownloaderBuilder {
    fn default() -> Self {
        Self {
            request_batch_size: 100,
            stream_batch_size: 1000,
            max_concurrent_requests: 100,
            max_buffered_responses: 10000,
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
    pub fn with_max_concurrent_requests(mut self, max_concurrent_requests: usize) -> Self {
        self.max_concurrent_requests = max_concurrent_requests;
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
            self.max_concurrent_requests,
            self.max_buffered_responses,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{generate_bodies, TestBodiesClient};
    use assert_matches::assert_matches;
    use futures_util::stream::StreamExt;
    use reth_db::{
        mdbx::{test_utils::create_test_db, EnvKind, WriteMap},
        transaction::DbTxMut,
    };
    use reth_eth_wire::BlockBody;
    use reth_interfaces::test_utils::{generators::random_header_range, TestConsensus};
    use reth_primitives::H256;
    use std::{collections::HashMap, sync::Arc};

    fn zip_blocks(
        headers: Vec<SealedHeader>,
        mut bodies: HashMap<H256, BlockBody>,
    ) -> Vec<BlockResponse> {
        headers
            .into_iter()
            .map(|header| {
                let body = bodies.remove(&header.hash()).unwrap();
                if header.is_empty() {
                    BlockResponse::Empty(header)
                } else {
                    BlockResponse::Full(SealedBlock {
                        header,
                        body: body.transactions,
                        ommers: body.ommers.into_iter().map(|o| o.seal()).collect(),
                    })
                }
            })
            .collect()
    }

    /// Check if future returns empty bodies without dispathing any requests.
    #[tokio::test]
    async fn request_returns_empty_bodies() {
        let headers = random_header_range(0..20, H256::zero());

        let client = Arc::new(TestBodiesClient::default());
        let fut = BodiesRequestFuture::new(client.clone(), Arc::new(TestConsensus::default()))
            .with_headers(headers.clone());

        assert_eq!(
            fut.await,
            headers.into_iter().map(|h| BlockResponse::Empty(h)).collect::<Vec<_>>()
        );
        assert_eq!(client.times_requested(), 0);
    }

    /// Check that the request future
    #[tokio::test]
    async fn request_retries_until_fullfilled() {
        // Generate some random blocks
        let (headers, bodies) = generate_bodies(0..20);

        let batch_size = 1;
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_max_batch_size(batch_size),
        );
        let fut = BodiesRequestFuture::new(client.clone(), Arc::new(TestConsensus::default()))
            .with_headers(headers.clone());

        assert_eq!(fut.await, zip_blocks(headers.clone(), bodies));
        assert_eq!(
            client.times_requested(),
            headers.into_iter().filter(|h| !h.is_empty()).count() as u64
        );
    }

    // Check that the blocks are emitted in order of block number, not in order of
    // first-downloaded
    #[tokio::test]
    async fn streams_bodies_in_order() {
        // Generate some random blocks
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let (headers, bodies) = generate_bodies(0..20);

        db.update(|tx| -> Result<(), db::Error> {
            for header in headers.iter() {
                tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
                tx.put::<tables::Headers>(header.num_hash().into(), header.clone().unseal())?;
            }
            Ok(())
        })
        .expect("failed to commit")
        .expect("failed to insert headers");

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let mut downloader = ConcurrentDownloaderBuilder::default().build(
            client.clone(),
            Arc::new(TestConsensus::default()),
            db,
        );
        downloader.set_header_range(0..20).expect("failed to set header range");

        assert_matches!(downloader.next().await, Some(res) => assert_eq!(res, zip_blocks(headers, bodies)));
        assert_eq!(client.times_requested(), 1);
    }
}
