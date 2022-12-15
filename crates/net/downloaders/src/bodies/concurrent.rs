use backon::{ExponentialBackoff, Retryable};
use futures_util::{stream, StreamExt, TryStreamExt};
use reth_interfaces::{
    consensus::Consensus as ConsensusTrait,
    p2p::{
        bodies::{client::BodiesClient, downloader::BodyDownloader},
        downloader::{DownloadStream, Downloader},
        error::{DownloadError, DownloadResult},
    },
};
use reth_primitives::{BlockLocked, SealedHeader};
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
    fn bodies_stream<'a, 'b, I>(&'a self, headers: I) -> DownloadStream<'a, BlockLocked>
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

    /// Fetch a batch of block bodies.
    async fn fetch_bodies(&self, headers: Vec<&SealedHeader>) -> DownloadResult<Vec<BlockLocked>> {
        let (peer_id, response) = self
            .client
            .get_block_body(headers.iter().map(|header| header.hash()).collect())
            .await?
            .split();

        response
            .into_iter()
            .zip(headers)
            .map(|(body, header)| {
                let block = BlockLocked {
                    header: header.clone(),
                    body: body.transactions,
                    ommers: body.ommers.into_iter().map(|header| header.seal()).collect(),
                };

                match self.consensus.pre_validate_block(&block) {
                    Ok(_) => Ok(block),
                    Err(error) => {
                        self.client.report_bad_message(peer_id);
                        Err(DownloadError::BlockValidation { hash: header.hash(), error })
                    }
                }
            })
            .collect()
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
    use reth_primitives::{PeerId, H256};
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
                .try_collect::<Vec<BlockLocked>>()
                .await,
            Ok(responses) => {
                assert_eq!(
                    responses,
                    headers
                        .into_iter()
                        .map(| header | {
                            let body = bodies .remove(&header.hash()).unwrap();
                            BlockLocked {
                                header,
                                body: body.transactions,
                                ommers: body.ommers.into_iter().map(|o| o.seal()).collect(),
                            }
                        })
                        .collect::<Vec<BlockLocked>>()
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

        assert_matches!(
            downloader.bodies_stream(&[SealedHeader::default()]).next().await,
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

        assert_matches!(
            downloader.bodies_stream(&[SealedHeader::default()]).next().await,
            Some(Ok(body)) => {
                assert_eq!(body, BlockLocked::default());
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
            downloader.bodies_stream(&[SealedHeader::default()]).next().await,
            Some(Err(DownloadError::RequestError(RequestError::Timeout)))
        );
        assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 2);
    }
}
