use futures::{Future, FutureExt};
use reth_eth_wire::BlockBody;
use reth_interfaces::{
    consensus::{Consensus as ConsensusTrait, Consensus},
    p2p::{
        bodies::{client::BodiesClient, response::BlockResponse},
        error::{DownloadError, PeerRequestResult},
    },
};
use reth_primitives::{PeerId, SealedBlock, SealedHeader, H256};
use std::{
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

type BodiesFut = Pin<Box<dyn Future<Output = PeerRequestResult<Vec<BlockBody>>> + Send>>;

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
/// If [BodiesRequestFuture] was initialized with all empty headers, no request will be dispatched
/// and they will be immediately returned upon polling.
///
/// NB: This assumes that peers respond with bodies in the order that they were requested.
/// This is a reasonable assumption to make as that's [what Geth
/// does](https://github.com/ethereum/go-ethereum/blob/f53ff0ff4a68ffc56004ab1d5cc244bcb64d3277/les/server_requests.go#L245).
/// All errors regarding the response cause the peer to get penalized, meaning that adversaries
/// that try to give us bodies that do not match the requested order are going to be penalized
/// and eventually disconnected.
pub(crate) struct BodiesRequestFuture<B> {
    client: Arc<B>,
    consensus: Arc<dyn Consensus>,
    // All requested headers
    headers: Vec<SealedHeader>,
    // Remaining hashes to download
    hashes_to_download: Vec<H256>,
    buffer: Vec<(PeerId, BlockBody)>,
    fut: Option<BodiesFut>,
}

impl<B> BodiesRequestFuture<B>
where
    B: BodiesClient + 'static,
{
    /// Returns an empty future. Use [BodiesRequestFuture::with_headers] to set the request.
    pub(crate) fn new(client: Arc<B>, consensus: Arc<dyn Consensus>) -> Self {
        Self {
            client,
            consensus,
            headers: Default::default(),
            hashes_to_download: Default::default(),
            buffer: Default::default(),
            fut: None,
        }
    }

    pub(crate) fn with_headers(mut self, headers: Vec<SealedHeader>) -> Self {
        self.headers = headers;
        self.reset_hashes();
        // Submit the request only if there are any headers to download.
        // Otherwise, the future will immediately be resolved.
        if !self.hashes_to_download.is_empty() {
            self.submit_request();
        }
        self
    }

    fn on_error(&mut self, error: DownloadError, peer_id: Option<PeerId>) {
        tracing::error!(target: "downloaders::bodies", ?peer_id, %error, "Error requesting bodies");
        if let Some(peer_id) = peer_id {
            self.client.report_bad_message(peer_id);
        }
        self.submit_request();
    }

    fn submit_request(&mut self) {
        let client = Arc::clone(&self.client);
        let request = self.hashes_to_download.clone();
        tracing::trace!(target: "downloaders::bodies", request_len = request.len(), "Requesting bodies");
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
    #[allow(clippy::result_large_err)]
    fn try_construct_blocks(&mut self) -> Result<Vec<BlockResponse>, (PeerId, DownloadError)> {
        // Drop the allocated memory for the buffer. Optimistically, it will not be reused.
        let mut bodies = std::mem::take(&mut self.buffer).into_iter();
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
                    (peer_id, DownloadError::BodyValidation { hash: header.hash(), error })
                })?;

                results.push(BlockResponse::Full(block));
            }
        }
        Ok(results)
    }
}

impl<B> Future for BodiesRequestFuture<B>
where
    B: BodiesClient + 'static,
{
    type Output = Vec<BlockResponse>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            // Check if there is a pending requests. It might not exist if all
            // headers are empty and there is nothing to download.
            if let Some(fut) = this.fut.as_mut() {
                match ready!(fut.poll_unpin(cx)) {
                    Ok(response) => {
                        let (peer_id, bodies) = response.split();
                        let request_len = this.hashes_to_download.len();
                        let response_len = bodies.len();
                        // Malicious peers often return a single block. Mark responses with single
                        // block when more than 1 were requested invalid.
                        // TODO: Instead of marking single block responses invalid, calculate
                        // soft response size lower limit and use that for filtering.
                        if bodies.is_empty() || (request_len != 1 && response_len == 1) {
                            this.on_error(DownloadError::EmptyResponse, Some(peer_id));
                            continue
                        }

                        if response_len > request_len {
                            this.on_error(
                                DownloadError::TooManyBodies {
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
                        // Draining the hashes here so that on the next `submit_request` call we
                        // only request the remaining bodies, instead of the
                        // ones we already received
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
                unreachable!("Body request logic failure")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::test_utils::zip_blocks,
        test_utils::{generate_bodies, TestBodiesClient},
    };
    use reth_interfaces::{
        p2p::bodies::response::BlockResponse,
        test_utils::{generators::random_header_range, TestConsensus},
    };
    use reth_primitives::H256;
    use std::sync::Arc;

    /// Check if future returns empty bodies without dispathing any requests.
    #[tokio::test]
    async fn request_returns_empty_bodies() {
        let headers = random_header_range(0..20, H256::zero());

        let client = Arc::new(TestBodiesClient::default());
        let fut = BodiesRequestFuture::new(client.clone(), Arc::new(TestConsensus::default()))
            .with_headers(headers.clone());

        assert_eq!(fut.await, headers.into_iter().map(BlockResponse::Empty).collect::<Vec<_>>());
        assert_eq!(client.times_requested(), 0);
    }

    /// Check that the request future
    #[tokio::test]
    async fn request_submits_until_fullfilled() {
        // Generate some random blocks
        let (headers, mut bodies) = generate_bodies(0..20);

        let batch_size = 2;
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_max_batch_size(batch_size),
        );
        let fut = BodiesRequestFuture::new(client.clone(), Arc::new(TestConsensus::default()))
            .with_headers(headers.clone());

        assert_eq!(fut.await, zip_blocks(headers.iter(), &mut bodies));
        assert_eq!(
            client.times_requested(),
            // div_ceild
            (headers.into_iter().filter(|h| !h.is_empty()).count() as u64 + 1) / 2
        );
    }
}
