use futures::{stream::FuturesUnordered, Future, FutureExt, Stream};
use futures_util::StreamExt;
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    consensus::{Consensus as ConsensusTrait, Error as ConsensusError},
    p2p::{
        bodies::{client::BodiesClient, downloader::BlockResponse},
        downloader::Downloader,
        error::{PeerRequestResult, RequestError},
    },
};
use reth_primitives::{BlockNumber, PeerId, SealedBlock, SealedHeader, H256};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use thiserror::Error;

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[derive(Debug)]
pub struct ConcurrentDownloader<B, C> {
    /// The bodies client
    client: Arc<B>,
    /// The consensus client
    consensus: Arc<C>,
    /// The batch size of non-empty blocks per one request
    request_batch_size: usize,
    /// The number of block bodies to return at once
    stream_batch_size: usize,
    /// The maximum number of requests to send concurrently.
    max_concurrent_requests: usize,
    /// Maximum amount of received headers to buffer internally.
    max_buffered_responses: usize,
    /// The first block number.
    first_block_number: BlockNumber, // TODO: Option
    /// The latest block number returned.
    latest_queued_block_number: Option<BlockNumber>,
    /// Headers to download bodies for.
    headers: VecDeque<SealedHeader>,
    /// requests in progress
    in_progress_queue: FuturesUnordered<BodiesRequestFuture<B, C>>,
    /// Buffered, unvalidated responses
    buffered_responses: BinaryHeap<OrderedBodiesResponse>,
    /// Queued body responses
    queued_bodies: Vec<BlockResponse>,
}

impl<B, C> ConcurrentDownloader<B, C>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
{
    fn new(
        client: Arc<B>,
        consensus: Arc<C>,
        request_batch_size: usize,
        stream_batch_size: usize,
        max_concurrent_requests: usize,
        max_buffered_responses: usize,
    ) -> Self {
        Self {
            client,
            consensus,
            request_batch_size,
            stream_batch_size,
            max_concurrent_requests,
            max_buffered_responses,
            first_block_number: 0,
            latest_queued_block_number: None,
            headers: Default::default(),
            in_progress_queue: Default::default(),
            buffered_responses: Default::default(),
            queued_bodies: Default::default(),
        }
    }

    /// Add headers to download
    pub fn add_headers(&mut self, mut headers: Vec<SealedHeader>) {
        headers.sort_by_key(|h| h.number);
        self.first_block_number = headers.first().map(|h| h.number).unwrap_or_default();
        // TODO: dedup and push instead resetting
        self.headers = VecDeque::from(headers);
    }

    fn next_request_fut(&mut self) -> Option<BodiesRequestFuture<B, C>> {
        let mut headers = Vec::default();
        let mut non_empty_headers = 0;
        while non_empty_headers < self.request_batch_size && !self.headers.is_empty() {
            let next_header = self.headers.pop_front().unwrap();
            if !next_header.is_empty() {
                non_empty_headers += 1;
            }
            headers.push(next_header);
        }

        if headers.is_empty() {
            None
        } else {
            Some(
                BodiesRequestFuture::new(Arc::clone(&self.client), Arc::clone(&self.consensus))
                    .with_headers(headers),
            )
        }
    }

    fn next_expected_block_number(&self) -> BlockNumber {
        match self.latest_queued_block_number {
            Some(num) => num + 1,
            None => self.first_block_number,
        }
    }

    fn has_capacity(&self) -> bool {
        self.in_progress_queue.len() < self.max_concurrent_requests &&
            self.buffered_responses.len() < self.max_buffered_responses
    }

    fn is_terminated(&self) -> bool {
        self.headers.is_empty() &&
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
    fn try_next_buffered(&mut self) -> Option<OrderedBodiesResponse> {
        if let Some(next) = self.buffered_responses.peek() {
            if next.block_number() == self.next_expected_block_number() {
                return self.buffered_responses.pop()
            }
        }
        None
    }
}

impl<Client, Consensus> Downloader for ConcurrentDownloader<Client, Consensus>
where
    Client: BodiesClient,
    Consensus: ConsensusTrait,
{
    type Client = Client;
    type Consensus = Consensus;

    fn client(&self) -> &Self::Client {
        self.client.borrow()
    }

    fn consensus(&self) -> &Self::Consensus {
        self.consensus.borrow()
    }
}

impl<Client, Consensus> Stream for ConcurrentDownloader<Client, Consensus>
where
    Client: BodiesClient + 'static,
    Consensus: ConsensusTrait + 'static,
{
    type Item = Vec<BlockResponse>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.is_terminated() {
            return Poll::Ready(None)
        }

        // yield next batch
        if this.queued_bodies.len() > this.stream_batch_size {
            let next_batch = this.queued_bodies.drain(..this.stream_batch_size);
            return Poll::Ready(Some(next_batch.collect()))
        }

        // this will submit new requests and poll them
        loop {
            // populate requests
            while this.has_capacity() {
                match this.next_request_fut() {
                    Some(fut) => this.in_progress_queue.push(fut),
                    None => break, // no more requests
                };
            }

            // Poll requests
            if let Poll::Ready(Some(response)) = this.in_progress_queue.poll_next_unpin(cx) {
                let response = OrderedBodiesResponse(response);
                if response.block_number() == this.next_expected_block_number() {
                    this.queue_bodies(response.0);
                    while let Some(buf_response) = this.try_next_buffered() {
                        this.queue_bodies(buf_response.0);
                    }
                } else {
                    this.buffered_responses.push(response);
                }
            } else {
                break
            }
        }

        // all requests are handled, stream is finished
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
unsafe impl<B, C> Sync for ConcurrentDownloader<B, C>
where
    B: BodiesClient,
    C: ConsensusTrait,
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
/// TODO: modify
/// Given a batch of headers, it proceeds to:
/// 1. Filter for all the non-empty headers
/// 2. Return early with the header values, if there were no non-empty headers, else..
/// 3. Request the bodies for the non-empty headers from a peer chosen by the network client
/// 4. For any non-empty headers, it proceeds to validate the corresponding body from the peer
/// and return it as part of the response via the [`BlockResponse::Full`] variant.
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
        let mut results = Vec::with_capacity(self.headers.len());
        for (idx, header) in self.headers.iter().cloned().enumerate() {
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

                        if bodies.len() > this.hashes_to_download.len() {
                            this.on_error(
                                BodyRequestError::TooManyBodies {
                                    expected: this.hashes_to_download.len(),
                                    received: bodies.len(),
                                },
                                Some(peer_id),
                            );
                            continue
                        }

                        tracing::trace!(
                            target: "downloaders::bodies", request_len = this.hashes_to_download.len(), response_len = bodies.len(), ?peer_id, "Received bodies"
                        );
                        this.hashes_to_download.drain(..bodies.len());
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
    fn block_number(&self) -> u64 {
        self.0.first().expect("is not empty").block_number()
    }
}

impl PartialEq for OrderedBodiesResponse {
    fn eq(&self, other: &Self) -> bool {
        self.block_number() == other.block_number()
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
        self.block_number().cmp(&other.block_number()).reverse()
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
            stream_batch_size: 100,
            max_concurrent_requests: 10,
            max_buffered_responses: 1000,
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
    pub fn build<B, C>(self, client: Arc<B>, consensus: Arc<C>) -> ConcurrentDownloader<B, C>
    where
        C: ConsensusTrait + 'static,
        B: BodiesClient + 'static,
    {
        ConcurrentDownloader::new(
            client,
            consensus,
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
    async fn returns_empty_bodies() {
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
        let (headers, bodies) = generate_bodies(0..20);

        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_should_delay(true),
        );
        let mut downloader = ConcurrentDownloaderBuilder::default()
            .build(client.clone(), Arc::new(TestConsensus::default()));
        downloader.add_headers(headers.clone());

        assert_matches!(downloader.next().await, Some(res) => assert_eq!(res, zip_blocks(headers, bodies)));
        assert_eq!(client.times_requested(), 1);
    }
}
