use futures::{stream::FuturesUnordered, Future, FutureExt, Stream};
use futures_util::StreamExt;
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    consensus::Consensus as ConsensusTrait,
    p2p::{
        bodies::{client::BodiesClient, downloader::BlockResponse},
        downloader::Downloader,
        error::{DownloadError, DownloadResult, PeerRequestResult},
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
    first_block_number: BlockNumber,
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

        if !headers.is_empty() {
            Some(
                BodiesRequestFuture::new(
                    Arc::clone(&self.client),
                    Arc::clone(&self.consensus),
                    headers,
                )
                .with_request(),
            )
        } else {
            None
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
}

struct BodiesRequestFuture<B, C> {
    client: Arc<B>,
    consensus: Arc<C>,
    headers: Vec<SealedHeader>,
    hashes_to_download: Vec<H256>,
    buffer: Vec<(PeerId, BlockBody)>,
    fut: Option<BodiesFut>,
}

impl<B, C> BodiesRequestFuture<B, C>
where
    B: BodiesClient + 'static,
    C: ConsensusTrait + 'static,
{
    fn new(client: Arc<B>, consensus: Arc<C>, headers: Vec<SealedHeader>) -> Self {
        Self {
            client,
            consensus,
            headers,
            hashes_to_download: Vec::default(),
            buffer: Vec::default(),
            fut: None,
        }
    }

    fn with_request(mut self) -> Self {
        self.reset_hashes();
        self.submit_request();
        self
    }

    fn on_error(&mut self, peer_id: PeerId, error: BodyRequestError) {
        tracing::error!(target: "downloaders::bodies", ?peer_id, request = ?self.hashes_to_download, %error, "Penalizing peer");
        self.client.report_bad_message(peer_id);
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
    fn try_construct_blocks(&mut self) -> Result<Vec<BlockResponse>, PeerId> {
        let mut bodies = self.buffer.drain(..);
        let mut results = Vec::with_capacity(self.headers.len());
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
                if let Err(error) = self.consensus.pre_validate_block(&block) {
                    tracing::error!(
                        target: "downloaders::bodies", ?peer_id, header = ?header.hash(), ?error, "Block validation error"
                    );
                    return Err(peer_id)
                }

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
            'inner: loop {
                // Future must exist at this point
                let response = ready!(this.fut.as_mut().unwrap().poll_unpin(cx));
                match response {
                    Ok(response) => {
                        let (peer_id, bodies) = response.split();
                        if bodies.is_empty() {
                            this.on_error(peer_id, BodyRequestError::EmptyResponse);
                            continue 'inner
                        }

                        if bodies.len() > this.hashes_to_download.len() {
                            this.on_error(
                                peer_id,
                                BodyRequestError::TooManyBodies {
                                    expected: this.hashes_to_download.len(),
                                    received: bodies.len(),
                                },
                            );
                            continue 'inner
                        }

                        tracing::trace!(
                            target: "downloaders::bodies", request_len = this.hashes_to_download.len(), response_len = bodies.len(), ?peer_id, "Received bodies"
                        );
                        this.hashes_to_download.drain(..bodies.len());
                        this.buffer.extend(bodies.into_iter().map(|b| (peer_id, b)));

                        if this.hashes_to_download.is_empty() {
                            // We are done
                            break 'inner
                        }

                        // Submit next request if not done
                        this.submit_request();
                    }
                    Err(error) => {
                        tracing::error!(target: "downloaders::bodies", request = ?this.hashes_to_download, ?error, "Request error");
                        this.submit_request();
                    }
                }
            }

            // Drain the buffer and attempt to construct the response.
            // If validation fails, the future will restart from scratch.
            match this.try_construct_blocks() {
                Ok(blocks) => return Poll::Ready(blocks),
                Err(error) => {
                    this.reset_hashes();
                    this.submit_request();
                }
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

// ===================================================

// TODO:
/// Given a batch of headers, this function proceeds to:
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
async fn fetch_bodies<B: BodiesClient, C: ConsensusTrait>(
    client: Arc<B>,
    consensus: Arc<C>,
    headers: Vec<&SealedHeader>,
) -> DownloadResult<Vec<BlockResponse>> {
    // Filter headers with transaction or ommers. These are the only ones
    // we will request
    let headers_to_download =
        headers.iter().filter(|h| !h.is_empty()).map(|h| h.hash()).collect::<Vec<_>>();
    if headers_to_download.is_empty() {
        tracing::trace!(target: "downloaders::bodies", len = headers.len(), "Nothing to download");
        return Ok(headers.into_iter().cloned().map(BlockResponse::Empty).collect())
    }

    // The bodies size might exceed a max response size limit set by peer. We need to keep
    // retrying until we finish downloading all of the requested bodies
    let mut responses = Vec::with_capacity(headers_to_download.len());
    while responses.len() != headers_to_download.len() {
        let request: Vec<_> = headers_to_download.iter().skip(responses.len()).cloned().collect();
        let request_len = request.len();
        tracing::trace!(target: "downloaders::bodies", request_len, "Requesting bodies");
        let (peer_id, bodies) = client.get_block_bodies(request.clone()).await?.split();

        if bodies.is_empty() {
            tracing::error!(
                target: "downloaders::bodies", ?peer_id, ?request, "Received empty response. Penalizing peer"
            );
            client.report_bad_message(peer_id);
            continue
        }

        tracing::trace!(
            target: "downloaders::bodies", request_len, response_len = bodies.len(), ?peer_id, "Received bodies"
        );
        bodies.into_iter().for_each(|b| responses.push((peer_id, b)));
    }

    let mut bodies = responses.into_iter();
    let mut results = Vec::with_capacity(headers.len());
    for header in headers.into_iter().cloned() {
        // If the header has no txs / ommers, just push it and continue
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
            consensus.pre_validate_block(&block).map_err(|error| {
                    tracing::trace!(
                        target: "downloaders::bodies", ?peer_id, header = ?header.hash(), ?error, request = ?headers_to_download, "Penalizing peer"
                    );
                    client.report_bad_message(peer_id);
                    DownloadError::BlockValidation { hash: header.hash(), error }
                })?;

            results.push(BlockResponse::Full(block));
        }
    }

    Ok(results)
}

// // TODO: Cleanup
// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::test_utils::{generate_bodies, TestBodiesClient};
//     use assert_matches::assert_matches;
//     use futures_util::stream::{StreamExt, TryStreamExt};
//     use reth_eth_wire::BlockBody;
//     use reth_interfaces::{
//         p2p::{bodies::downloader::BodyDownloader, error::RequestError},
//         test_utils::TestConsensus,
//     };
//     use reth_primitives::{Header, PeerId, H256};
//     use std::{
//         sync::{
//             atomic::{AtomicUsize, Ordering},
//             Arc,
//         },
//         time::Duration,
//     };

//     // Check that the blocks are emitted in order of block number, not in order of
//     // first-downloaded
//     #[tokio::test]
//     async fn emits_bodies_in_order() {
//         // Generate some random blocks
//         let (headers, mut bodies) = generate_bodies(0..20);

//         let downloader = ConcurrentDownloader::new(
//             Arc::new(TestBodiesClient::new(|hashes: Vec<H256>| {
//                 let mut bodies = bodies.clone();
//                 async move {
//                     // Simulate that the request for this (random) block takes 0-100ms
//                     tokio::time::sleep(Duration::from_millis(hashes[0].to_low_u64_be() % 100))
//                         .await;

//                     Ok((
//                         PeerId::default(),
//                         hashes
//                             .into_iter()
//                             .map(|hash| {
//                                 bodies
//                                     .remove(&hash)
//                                     .expect("Downloader asked for a block it should not ask for")
//                             })
//                             .collect(),
//                     )
//                         .into())
//                 }
//             })),
//             Arc::new(TestConsensus::default()),
//         );

//         assert_matches!(
//             downloader
//                 .bodies_stream(headers.clone().iter())
//                 .try_collect::<Vec<BlockResponse>>()
//                 .await,
//             Ok(responses) => {
//                 assert_eq!(
//                     responses,
//                     headers
//                         .into_iter()
//                         .map(|header| {
//                             let body = bodies.remove(&header.hash()).unwrap();
//                             if header.is_empty() {
//                                 BlockResponse::Empty(header)
//                             } else {
//                                 BlockResponse::Full(SealedBlock {
//                                     header,
//                                     body: body.transactions,
//                                     ommers: body.ommers.into_iter().map(|o| o.seal()).collect(),
//                                 })
//                             }
//                         })
//                         .collect::<Vec<BlockResponse>>()
//                 );
//             }
//         );
//     }

//     /// Checks that non-retryable errors bubble up
//     #[tokio::test]
//     async fn client_failure() {
//         let downloader = ConcurrentDownloader::new(
//             Arc::new(TestBodiesClient::new(|_: Vec<H256>| async {
//                 Err(RequestError::ChannelClosed)
//             })),
//             Arc::new(TestConsensus::default()),
//         );

//         let headers = &[Header { ommers_hash: H256::default(), ..Default::default() }.seal()];
//         let mut stream = downloader.bodies_stream(headers);
//         assert_matches!(
//             stream.next().await,
//             Some(Err(DownloadError::RequestError(RequestError::ChannelClosed)))
//         );
//     }

//     /// Checks that the body request is retried on timeouts
//     #[tokio::test]
//     async fn retries_timeouts() {
//         let retries_left = Arc::new(AtomicUsize::new(3));
//         let downloader = ConcurrentDownloader::new(
//             Arc::new(TestBodiesClient::new(|mut header_hash: Vec<H256>| {
//                 let retries_left = retries_left.clone();
//                 let _header_hash = header_hash.remove(0);
//                 async move {
//                     if retries_left.load(Ordering::SeqCst) > 0 {
//                         retries_left.fetch_sub(1, Ordering::SeqCst);
//                         Err(RequestError::Timeout)
//                     } else {
//                         Ok((
//                             PeerId::default(),
//                             vec![BlockBody { transactions: vec![], ommers: vec![] }],
//                         )
//                             .into())
//                     }
//                 }
//             })),
//             Arc::new(TestConsensus::default()),
//         );

//         let header = Header { ommers_hash: H256::default(), ..Default::default() }.seal();
//         assert_matches!(
//             downloader.bodies_stream(&[header.clone()]).next().await,
//             Some(Ok(BlockResponse::Full(block))) => {
//                 assert_eq!(block.header, header);
//             }
//         );
//         assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 0);
//     }

//     /// Checks that the timeout error bubbles up if we've retried too many times
//     #[tokio::test]
//     async fn too_many_retries() {
//         let retries_left = Arc::new(AtomicUsize::new(3));
//         let downloader = ConcurrentDownloader::new(
//             Arc::new(TestBodiesClient::new(|mut header_hash: Vec<H256>| {
//                 let _header_hash = header_hash.remove(0);
//                 let retries_left = retries_left.clone();
//                 async move {
//                     if retries_left.load(Ordering::SeqCst) > 0 {
//                         retries_left.fetch_sub(1, Ordering::SeqCst);
//                         Err(RequestError::Timeout)
//                     } else {
//                         Ok((
//                             PeerId::default(),
//                             vec![BlockBody { transactions: vec![], ommers: vec![] }],
//                         )
//                             .into())
//                     }
//                 }
//             })),
//             Arc::new(TestConsensus::default()),
//         )
//         .with_retries(0);

//         assert_matches!(
//             downloader
//                 .bodies_stream(&[
//                     Header { ommers_hash: H256::default(), ..Default::default() }.seal()
//                 ])
//                 .next()
//                 .await,
//             Some(Err(DownloadError::RequestError(RequestError::Timeout)))
//         );
//         assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 2);
//     }

//     /// Checks that the downloader keeps retrying the request in case
//     /// it receives too few bodies
//     #[tokio::test]
//     async fn received_few_bodies() {
//         // Generate some random blocks
//         let response_len = 5;
//         let (headers, mut bodies) = generate_bodies(0..20);

//         let download_attempt = Arc::new(AtomicUsize::new(0));
//         let downloader = ConcurrentDownloader::new(
//             Arc::new(TestBodiesClient::new(|hashes: Vec<H256>| {
//                 let mut bodies = bodies.clone();
//                 let download_attempt = download_attempt.clone();
//                 async move {
//                     // Simulate that the request for this (random) block takes 0-100ms
//                     tokio::time::sleep(Duration::from_millis(hashes[0].to_low_u64_be() % 100))
//                         .await;

//                     let attempt = download_attempt.load(Ordering::SeqCst);
//                     let results = hashes
//                         .into_iter()
//                         .take(response_len)
//                         .map(|hash| {
//                             bodies
//                                 .remove(&hash)
//                                 .expect("Downloader asked for a block it should not ask for")
//                         })
//                         .collect();
//                     download_attempt.store(attempt + 1, Ordering::SeqCst);
//                     Ok((PeerId::default(), results).into())
//                 }
//             })),
//             Arc::new(TestConsensus::default()),
//         );
//         assert_matches!(
//             downloader
//                 .bodies_stream(headers.iter())
//                 .try_collect::<Vec<BlockResponse>>()
//                 .await,
//             Ok(responses) => {
//                 assert_eq!(
//                     responses,
//                     headers
//                         .into_iter()
//                         .map(|header| {
//                             let body = bodies.remove(&header.hash()).unwrap();
//                             if header.is_empty() {
//                                 BlockResponse::Empty(header)
//                             } else {
//                                 BlockResponse::Full(SealedBlock {
//                                     header,
//                                     body: body.transactions,
//                                     ommers: body.ommers.into_iter().map(|o| o.seal()).collect(),
//                                 })
//                             }
//                         })
//                         .collect::<Vec<BlockResponse>>()
//                 );
//             }
//         );
//     }

//     /// Checks that the downloader bubbles up the error after attempting to re-request bodies
//     #[tokio::test]
//     async fn received_few_bodies_and_empty() {
//         // Generate some random blocks
//         let response_len = 5;
//         let (headers, bodies) = generate_bodies(0..20);

//         let download_attempt = Arc::new(AtomicUsize::new(0));
//         let downloader = ConcurrentDownloader::new(
//             Arc::new(TestBodiesClient::new(|hashes: Vec<H256>| {
//                 let mut bodies = bodies.clone();
//                 let download_attempt = download_attempt.clone();
//                 async move {
//                     // Simulate that the request for this (random) block takes 0-100ms
//                     tokio::time::sleep(Duration::from_millis(hashes[0].to_low_u64_be() % 100))
//                         .await;

//                     if hashes.len() <= response_len {
//                         return Ok((PeerId::default(), vec![]).into())
//                     }

//                     let attempt = download_attempt.load(Ordering::SeqCst);
//                     let results = hashes
//                         .into_iter()
//                         .take(response_len)
//                         .map(|hash| {
//                             bodies
//                                 .remove(&hash)
//                                 .expect("Downloader asked for a block it should not ask for")
//                         })
//                         .collect();
//                     download_attempt.store(attempt + 1, Ordering::SeqCst);
//                     Ok((PeerId::default(), results).into())
//                 }
//             })),
//             Arc::new(TestConsensus::default()),
//         );
//         assert_matches!(
//             downloader.bodies_stream(headers.iter()).try_collect::<Vec<BlockResponse>>().await,
//             Err(DownloadError::RequestError(RequestError::BadResponse))
//         );
//     }
// }
