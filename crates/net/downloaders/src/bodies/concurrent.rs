use backon::{ExponentialBackoff, Retryable};
use futures_util::{stream, StreamExt, TryStreamExt};
use reth_interfaces::{
    consensus::Consensus as ConsensusTrait,
    p2p::{
        bodies::{
            client::BodiesClient,
            downloader::{BlockResponse, BodyDownloader},
        },
        downloader::{DownloadStream, Downloader},
        error::{DownloadError, DownloadResult, RequestError},
    },
};
use reth_primitives::{SealedBlock, SealedHeader};
use std::{borrow::Borrow, sync::Arc};

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[derive(Debug)]
pub struct ConcurrentDownloader<Client, Consensus> {
    /// The bodies client
    client: Arc<Client>,
    /// The consensus client
    consensus: Arc<Consensus>,
    /// The number of retries for each request.
    retries: usize,
    /// The batch size per one request
    batch_size: usize,
    /// The maximum number of requests to send concurrently.
    concurrency: usize,
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

impl<Client, Consensus> BodyDownloader for ConcurrentDownloader<Client, Consensus>
where
    Client: BodiesClient,
    Consensus: ConsensusTrait,
{
    fn bodies_stream<'a, 'b, I>(&'a self, headers: I) -> DownloadStream<'a, BlockResponse>
    where
        I: IntoIterator<Item = &'b SealedHeader>,
        <I as IntoIterator>::IntoIter: Send + 'b,
        'b: 'a,
    {
        Box::pin(
            stream::iter(headers.into_iter())
                .chunks(self.batch_size)
                .map(move |headers| {
                    (move || self.fetch_bodies(headers.clone()))
                        .retry(ExponentialBackoff::default().with_max_times(self.retries))
                })
                .buffered(self.concurrency)
                .map_ok(|blocks| stream::iter(blocks.into_iter()).map(Ok))
                .try_flatten(),
        )
    }
}

impl<Client, Consensus> ConcurrentDownloader<Client, Consensus>
where
    Client: BodiesClient,
    Consensus: ConsensusTrait,
{
    /// Create a new concurrent downloader instance.
    pub fn new(client: Arc<Client>, consensus: Arc<Consensus>) -> Self {
        Self { client, consensus, retries: 3, batch_size: 100, concurrency: 5 }
    }

    /// Set the number of blocks to fetch at the same time.
    ///
    /// Defaults to 100.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Set the maximum number of requests to send concurrently.
    ///
    /// Defaults to 5.
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// Set the number of times to retry body fetch requests.
    ///
    /// Defaults to 3.
    pub fn with_retries(mut self, retries: usize) -> Self {
        self.retries = retries;
        self
    }

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
    async fn fetch_bodies(
        &self,
        headers: Vec<&SealedHeader>,
    ) -> DownloadResult<Vec<BlockResponse>> {
        // Filter headers with transaction or ommers. These are the only ones
        // we will request
        let request =
            headers.iter().filter(|h| !h.is_empty()).map(|h| h.hash()).collect::<Vec<_>>();
        if request.is_empty() {
            tracing::trace!(target: "downloaders::bodies", len = headers.len(), "Nothing to download");
            return Ok(headers.into_iter().cloned().map(BlockResponse::Empty).collect())
        }

        let request_len = request.len();
        tracing::trace!(target: "downloaders::bodies", request_len, "Requesting bodies");
        let (peer_id, bodies) = self.client.get_block_bodies(request.clone()).await?.split();
        tracing::trace!(
            target: "downloaders::bodies", request_len, response_len = bodies.len(), ?peer_id, "Received bodies"
        );

        let mut bodies = bodies.into_iter();

        let mut responses = Vec::with_capacity(headers.len());
        for header in headers.into_iter().cloned() {
            // If the header has no txs / ommers, just push it and continue
            if header.is_empty() {
                responses.push(BlockResponse::Empty(header));
            } else {
                // If the header was not empty, and there is no body, then
                // the peer gave us a bad resopnse and we should return
                // error.
                let body = match bodies.next() {
                    Some(body) => body,
                    None => {
                        tracing::trace!(
                            target: "downloaders::bodies", ?peer_id, header = ?header.hash(), ?request, "Penalizing peer"
                        );
                        self.client.report_bad_message(peer_id);
                        // TODO: We error always, this means that if we got no body from a peer
                        // and the header was not empty we will discard a bunch of progress.
                        // This will cause redundant bandwdith usage (due to how our retriable
                        // stream works via std::iter), but we will address this
                        // with a custom retriable stream that maintains state and is able to
                        // "resume" progress from the current state of `responses`.
                        return Err(DownloadError::RequestError(RequestError::BadResponse))
                    }
                };

                let block = SealedBlock {
                    header: header.clone(),
                    body: body.transactions,
                    ommers: body.ommers.into_iter().map(|header| header.seal()).collect(),
                };

                // This ensures that the TxRoot and OmmersRoot from the header match the
                // ones calculated manually from the block body.
                self.consensus.pre_validate_block(&block).map_err(|error| {
                    tracing::trace!(
                        target: "downloaders::bodies", ?peer_id, header = ?header.hash(), ?error, ?request, "Penalizing peer"
                    );
                    self.client.report_bad_message(peer_id);
                    DownloadError::BlockValidation { hash: header.hash(), error }
                })?;

                responses.push(BlockResponse::Full(block));
            }
        }

        Ok(responses)
    }
}

// TODO: Cleanup
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{generate_bodies, TestBodiesClient};
    use assert_matches::assert_matches;
    use futures_util::stream::{StreamExt, TryStreamExt};
    use reth_eth_wire::BlockBody;
    use reth_interfaces::{
        p2p::{bodies::downloader::BodyDownloader, error::RequestError},
        test_utils::TestConsensus,
    };
    use reth_primitives::{Header, PeerId, H256};
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    // Check that the blocks are emitted in order of block number, not in order of
    // first-downloaded
    #[tokio::test]
    async fn emits_bodies_in_order() {
        // Generate some random blocks
        let (headers, mut bodies) = generate_bodies(0..20);

        let downloader = ConcurrentDownloader::new(
            Arc::new(TestBodiesClient::new(|hashes: Vec<H256>| {
                let mut bodies = bodies.clone();
                async move {
                    // Simulate that the request for this (random) block takes 0-100ms
                    tokio::time::sleep(Duration::from_millis(hashes[0].to_low_u64_be() % 100))
                        .await;

                    Ok((
                        PeerId::default(),
                        hashes
                            .into_iter()
                            .map(|hash| {
                                bodies
                                    .remove(&hash)
                                    .expect("Downloader asked for a block it should not ask for")
                            })
                            .collect(),
                    )
                        .into())
                }
            })),
            Arc::new(TestConsensus::default()),
        );

        assert_matches!(
            downloader
                .bodies_stream(headers.clone().iter())
                .try_collect::<Vec<BlockResponse>>()
                .await,
            Ok(responses) => {
                assert_eq!(
                    responses,
                    headers
                        .into_iter()
                        .map(| header | {
                            let body = bodies .remove(&header.hash()).unwrap();
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
                        .collect::<Vec<BlockResponse>>()
                );
            }
        );
    }

    /// Checks that non-retryable errors bubble up
    #[tokio::test]
    async fn client_failure() {
        let downloader = ConcurrentDownloader::new(
            Arc::new(TestBodiesClient::new(|_: Vec<H256>| async {
                Err(RequestError::ChannelClosed)
            })),
            Arc::new(TestConsensus::default()),
        );

        let headers = &[Header { ommers_hash: H256::default(), ..Default::default() }.seal()];
        let mut stream = downloader.bodies_stream(headers);
        assert_matches!(
            stream.next().await,
            Some(Err(DownloadError::RequestError(RequestError::ChannelClosed)))
        );
    }

    /// Checks that the body request is retried on timeouts
    #[tokio::test]
    async fn retries_timeouts() {
        let retries_left = Arc::new(AtomicUsize::new(3));
        let downloader = ConcurrentDownloader::new(
            Arc::new(TestBodiesClient::new(|mut header_hash: Vec<H256>| {
                let retries_left = retries_left.clone();
                let _header_hash = header_hash.remove(0);
                async move {
                    if retries_left.load(Ordering::SeqCst) > 0 {
                        retries_left.fetch_sub(1, Ordering::SeqCst);
                        Err(RequestError::Timeout)
                    } else {
                        Ok((
                            PeerId::default(),
                            vec![BlockBody { transactions: vec![], ommers: vec![] }],
                        )
                            .into())
                    }
                }
            })),
            Arc::new(TestConsensus::default()),
        );

        let header = Header { ommers_hash: H256::default(), ..Default::default() }.seal();
        assert_matches!(
            downloader.bodies_stream(&[header.clone()]).next().await,
            Some(Ok(BlockResponse::Full(block))) => {
                assert_eq!(block.header, header);
            }
        );
        assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 0);
    }

    /// Checks that the timeout error bubbles up if we've retried too many times
    #[tokio::test]
    async fn too_many_retries() {
        let retries_left = Arc::new(AtomicUsize::new(3));
        let downloader = ConcurrentDownloader::new(
            Arc::new(TestBodiesClient::new(|mut header_hash: Vec<H256>| {
                let _header_hash = header_hash.remove(0);
                let retries_left = retries_left.clone();
                async move {
                    if retries_left.load(Ordering::SeqCst) > 0 {
                        retries_left.fetch_sub(1, Ordering::SeqCst);
                        Err(RequestError::Timeout)
                    } else {
                        Ok((
                            PeerId::default(),
                            vec![BlockBody { transactions: vec![], ommers: vec![] }],
                        )
                            .into())
                    }
                }
            })),
            Arc::new(TestConsensus::default()),
        )
        .with_retries(0);

        assert_matches!(
            downloader
                .bodies_stream(&[
                    Header { ommers_hash: H256::default(), ..Default::default() }.seal()
                ])
                .next()
                .await,
            Some(Err(DownloadError::RequestError(RequestError::Timeout)))
        );
        assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 2);
    }
}
