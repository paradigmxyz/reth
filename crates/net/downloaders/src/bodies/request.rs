use crate::metrics::DownloaderMetrics;
use futures::{Future, FutureExt};
use reth_interfaces::{
    consensus::{Consensus as ConsensusTrait, Consensus},
    p2p::{
        bodies::{client::BodiesClient, response::BlockResponse},
        error::{DownloadError, DownloadResult},
        priority::Priority,
    },
};
use reth_primitives::{BlockBody, PeerId, SealedBlock, SealedHeader, WithPeerId, H256};
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

/// Body request implemented as a [Future].
///
/// The future will poll the underlying request until fulfilled.
/// If the response arrived with insufficient number of bodies, the future
/// will issue another request until all bodies are collected.
///
/// It then proceeds to verify the downloaded bodies. In case of an validation error,
/// the future will start over.
///
/// The future will filter out any empty headers (see [reth_primitives::Header::is_empty]) from the
/// request. If [BodiesRequestFuture] was initialized with all empty headers, no request will be
/// dispatched and they will be immediately returned upon polling.
///
/// NB: This assumes that peers respond with bodies in the order that they were requested.
/// This is a reasonable assumption to make as that's [what Geth
/// does](https://github.com/ethereum/go-ethereum/blob/f53ff0ff4a68ffc56004ab1d5cc244bcb64d3277/les/server_requests.go#L245).
/// All errors regarding the response cause the peer to get penalized, meaning that adversaries
/// that try to give us bodies that do not match the requested order are going to be penalized
/// and eventually disconnected.
pub(crate) struct BodiesRequestFuture<B: BodiesClient> {
    client: Arc<B>,
    consensus: Arc<dyn Consensus>,
    metrics: DownloaderMetrics,
    priority: Priority,
    // Headers to download. The collection is shrunk as responses are buffered.
    headers: VecDeque<SealedHeader>,
    buffer: Vec<BlockResponse>,
    fut: Option<B::Output>,
    last_request_len: Option<usize>,
}

impl<B> BodiesRequestFuture<B>
where
    B: BodiesClient + 'static,
{
    /// Returns an empty future. Use [BodiesRequestFuture::with_headers] to set the request.
    pub(crate) fn new(
        client: Arc<B>,
        consensus: Arc<dyn Consensus>,
        priority: Priority,
        metrics: DownloaderMetrics,
    ) -> Self {
        Self {
            client,
            consensus,
            metrics,
            priority,
            headers: Default::default(),
            buffer: Default::default(),
            last_request_len: None,
            fut: None,
        }
    }

    pub(crate) fn with_headers(mut self, headers: Vec<SealedHeader>) -> Self {
        self.buffer.reserve_exact(headers.len());
        self.headers = VecDeque::from(headers);
        // Submit the request only if there are any headers to download.
        // Otherwise, the future will immediately be resolved.
        if let Some(req) = self.next_request() {
            self.submit_request(req);
        }
        self
    }

    fn on_error(&mut self, error: DownloadError, peer_id: Option<PeerId>) {
        self.metrics.increment_errors(&error);
        tracing::error!(target: "downloaders::bodies", ?peer_id, %error, "Error requesting bodies");
        if let Some(peer_id) = peer_id {
            self.client.report_bad_message(peer_id);
        }
        self.submit_request(self.next_request().expect("existing hashes to resubmit"));
    }

    /// Retrieve header hashes for the next request.
    fn next_request(&self) -> Option<Vec<H256>> {
        let mut hashes = self.headers.iter().filter(|h| !h.is_empty()).map(|h| h.hash()).peekable();
        hashes.peek().is_some().then(|| hashes.collect())
    }

    /// Submit the request.
    fn submit_request(&mut self, req: Vec<H256>) {
        tracing::trace!(target: "downloaders::bodies", request_len = req.len(), "Requesting bodies");
        let client = Arc::clone(&self.client);
        self.last_request_len = Some(req.len());
        self.fut = Some(client.get_block_bodies_with_priority(req, self.priority));
    }

    /// Process block response.
    /// Returns an error if the response is invalid.
    fn on_block_response(&mut self, response: WithPeerId<Vec<BlockBody>>) -> DownloadResult<()> {
        let (peer_id, bodies) = response.split();
        let request_len = self.last_request_len.unwrap_or_default();
        let response_len = bodies.len();

        tracing::trace!(target: "downloaders::bodies", request_len, response_len, ?peer_id, "Received bodies");

        // Increment total downloaded metric
        self.metrics.total_downloaded.increment(response_len as u64);

        // Malicious peers often return a single block. Mark responses with single
        // block when more than 1 were requested invalid.
        // TODO: Instead of marking single block responses invalid, calculate
        // soft response size lower limit and use that for filtering.
        if bodies.is_empty() || (request_len != 1 && response_len == 1) {
            return Err(DownloadError::EmptyResponse)
        }

        if response_len > request_len {
            return Err(DownloadError::TooManyBodies {
                expected: request_len,
                received: response_len,
            })
        }

        // Buffer block responses
        self.try_buffer_blocks(bodies)?;

        // Submit next request if any
        if let Some(req) = self.next_request() {
            self.submit_request(req);
        } else {
            self.fut = None;
        }

        Ok(())
    }

    /// Attempt to buffer body responses. Returns an error if body response fails validation.
    /// Every body preceeding the failed one will be buffered.
    ///
    /// This method removes headers from the internal collection.
    /// If the response fails validation, then the header will be put back.
    fn try_buffer_blocks(&mut self, bodies: Vec<BlockBody>) -> DownloadResult<()> {
        let mut bodies = bodies.into_iter().peekable();

        while bodies.peek().is_some() {
            let next_header = match self.headers.pop_front() {
                Some(header) => header,
                None => return Ok(()), // no more headers
            };

            if next_header.is_empty() {
                self.buffer.push(BlockResponse::Empty(next_header));
            } else {
                let next_body = bodies.next().unwrap();
                let block = SealedBlock {
                    header: next_header,
                    body: next_body.transactions,
                    ommers: next_body.ommers.into_iter().map(|h| h.seal_slow()).collect(),
                    withdrawals: next_body.withdrawals,
                };

                if let Err(error) = self.consensus.pre_validate_block(&block) {
                    // Put the header back and return an error
                    let hash = block.hash();
                    self.headers.push_front(block.header);
                    return Err(DownloadError::BodyValidation { hash, error })
                }

                self.buffer.push(BlockResponse::Full(block));
            }
        }

        Ok(())
    }
}

impl<B> Future for BodiesRequestFuture<B>
where
    B: BodiesClient + 'static,
{
    type Output = DownloadResult<Vec<BlockResponse>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            if this.headers.is_empty() {
                return Poll::Ready(Ok(std::mem::take(&mut this.buffer)))
            }

            // Check if there is a pending requests. It might not exist if all
            // headers are empty and there is nothing to download.
            if let Some(fut) = this.fut.as_mut() {
                match ready!(fut.poll_unpin(cx)) {
                    Ok(response) => {
                        let peer_id = response.peer_id();
                        if let Err(error) = this.on_block_response(response) {
                            this.on_error(error, Some(peer_id));
                        }
                    }
                    Err(error) => {
                        if error.is_channel_closed() {
                            return Poll::Ready(Err(error.into()))
                        }

                        this.on_error(error.into(), None);
                    }
                }
            }

            // Buffer any empty headers
            while this.headers.front().map(|h| h.is_empty()).unwrap_or_default() {
                let header = this.headers.pop_front().unwrap();
                this.buffer.push(BlockResponse::Empty(header));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bodies::test_utils::zip_blocks,
        test_utils::{generate_bodies, TestBodiesClient, TEST_SCOPE},
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
        let fut = BodiesRequestFuture::new(
            client.clone(),
            Arc::new(TestConsensus::default()),
            Priority::Normal,
            DownloaderMetrics::new(TEST_SCOPE),
        )
        .with_headers(headers.clone());

        assert_eq!(
            fut.await.unwrap(),
            headers.into_iter().map(BlockResponse::Empty).collect::<Vec<_>>()
        );
        assert_eq!(client.times_requested(), 0);
    }

    /// Check that the request future
    #[tokio::test]
    async fn request_submits_until_fulfilled() {
        // Generate some random blocks
        let (headers, mut bodies) = generate_bodies(0..20);

        let batch_size = 2;
        let client = Arc::new(
            TestBodiesClient::default().with_bodies(bodies.clone()).with_max_batch_size(batch_size),
        );
        let fut = BodiesRequestFuture::new(
            client.clone(),
            Arc::new(TestConsensus::default()),
            Priority::Normal,
            DownloaderMetrics::new(TEST_SCOPE),
        )
        .with_headers(headers.clone());

        assert_eq!(fut.await.unwrap(), zip_blocks(headers.iter(), &mut bodies));
        assert_eq!(
            client.times_requested(),
            // div_ceild
            (headers.into_iter().filter(|h| !h.is_empty()).count() as u64 + 1) / 2
        );
    }
}
