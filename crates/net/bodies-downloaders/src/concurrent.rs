use backon::{ExponentialBackoff, Retryable};
use futures_util::{stream, StreamExt};
use reth_eth_wire::BlockBody;
use reth_interfaces::p2p::bodies::{
    client::BodiesClient,
    downloader::{BodiesStream, BodyDownloader},
    error::{BodiesClientError, DownloadError},
};
use reth_primitives::{BlockNumber, H256};
use std::sync::Arc;

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[derive(Debug)]
pub struct ConcurrentDownloader<C> {
    /// The bodies client
    client: Arc<C>,
    /// The number of retries for each request.
    retries: usize,
    /// The batch size per one request
    batch_size: usize,
}

impl<C: BodiesClient> BodyDownloader for ConcurrentDownloader<C> {
    type Client = C;

    /// The block bodies client
    fn client(&self) -> &Self::Client {
        &self.client
    }

    fn bodies_stream<'a, 'b, I>(&'a self, headers: I) -> BodiesStream<'a>
    where
        I: IntoIterator<Item = &'b (BlockNumber, H256)>,
        <I as IntoIterator>::IntoIter: Send + 'b,
        'b: 'a,
    {
        Box::pin(
            stream::iter(headers.into_iter().map(|(block_number, header_hash)| {
                (|| self.fetch_body(*block_number, *header_hash))
                    .retry(ExponentialBackoff::default().with_max_times(self.retries))
                    .when(|err| err.is_retryable())
            }))
            .buffered(self.batch_size),
        )
    }
}

impl<C: BodiesClient> ConcurrentDownloader<C> {
    /// Create a new concurrent downloader instance.
    pub fn new(client: Arc<C>) -> Self {
        Self { client, retries: 3, batch_size: 100 }
    }

    /// Set the number of blocks to fetch at the same time.
    ///
    /// Defaults to 100.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Set the number of times to retry body fetch requests.
    ///
    /// Defaults to 3.
    pub fn with_retries(mut self, retries: usize) -> Self {
        self.retries = retries;
        self
    }

    /// Fetch a single block body.
    async fn fetch_body(
        &self,
        block_number: BlockNumber,
        header_hash: H256,
    ) -> Result<(BlockNumber, H256, BlockBody), DownloadError> {
        match self.client.get_block_body(header_hash).await {
            Ok(body) => Ok((block_number, header_hash, body)),
            Err(err) => Err(match err {
                BodiesClientError::Timeout { header_hash } => {
                    DownloadError::Timeout { header_hash }
                }
                err => DownloadError::Client { source: err },
            }),
        }
    }
}

// TODO: Cleanup
#[cfg(test)]
mod tests {
    use super::*;
    use crate::concurrent::{
        tests::test_utils::{generate_bodies, TestClient},
        ConcurrentDownloader,
    };
    use assert_matches::assert_matches;
    use futures_util::stream::{StreamExt, TryStreamExt};
    use reth_interfaces::p2p::{
        bodies::{downloader::BodyDownloader, error::BodiesClientError},
        error::RequestError,
    };
    use reth_primitives::H256;
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
        let (hashes, mut bodies) = generate_bodies(0..20);

        let downloader = ConcurrentDownloader::new(Arc::new(TestClient::new(|hash: H256| {
            let mut bodies = bodies.clone();
            async move {
                // Simulate that the request for this (random) block takes 0-100ms
                tokio::time::sleep(Duration::from_millis(hash.to_low_u64_be() % 100)).await;

                Ok(bodies
                    .remove(&hash)
                    .expect("Downloader asked for a block it should not ask for"))
            }
        })));

        assert_matches!(
            downloader
                .bodies_stream(hashes.iter())
                .try_collect::<Vec<(BlockNumber, H256, BlockBody)>>()
                .await,
            Ok(responses) => {
                assert_eq!(
                    responses,
                    hashes
                        .into_iter()
                        .map(|(num, hash)| {
                            (num, hash, bodies.remove(&hash).unwrap())
                        })
                        .collect::<Vec<(BlockNumber, H256, BlockBody)>>()
                );
            }
        );
    }

    /// Checks that non-retryable errors bubble up
    #[tokio::test]
    async fn client_failure() {
        let downloader = ConcurrentDownloader::new(Arc::new(TestClient::new(|_: H256| async {
            Err(BodiesClientError::Internal(RequestError::ChannelClosed))
        })));

        assert_matches!(
            downloader.bodies_stream(&[(0, H256::zero())]).next().await,
            Some(Err(DownloadError::Client {
                source: BodiesClientError::Internal(RequestError::ChannelClosed)
            }))
        );
    }

    /// Checks that the body request is retried on timeouts
    #[tokio::test]
    async fn retries_timeouts() {
        let retries_left = Arc::new(AtomicUsize::new(3));
        let downloader =
            ConcurrentDownloader::new(Arc::new(TestClient::new(|header_hash: H256| {
                let retries_left = retries_left.clone();
                async move {
                    if retries_left.load(Ordering::SeqCst) > 0 {
                        retries_left.fetch_sub(1, Ordering::SeqCst);
                        Err(BodiesClientError::Timeout { header_hash })
                    } else {
                        Ok(BlockBody { transactions: vec![], ommers: vec![] })
                    }
                }
            })));

        assert_matches!(
            downloader.bodies_stream(&[(0, H256::zero())]).next().await,
            Some(Ok(body)) => {
                assert_eq!(body.0, 0);
                assert_eq!(body.1, H256::zero());
                assert_eq!(body.2, BlockBody {
                    transactions: vec![],
                    ommers: vec![]
                })
            }
        );
        assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 0);
    }

    /// Checks that the timeout error bubbles up if we've retried too many times
    #[tokio::test]
    async fn too_many_retries() {
        let retries_left = Arc::new(AtomicUsize::new(3));
        let downloader =
            ConcurrentDownloader::new(Arc::new(TestClient::new(|header_hash: H256| {
                let retries_left = retries_left.clone();
                async move {
                    if retries_left.load(Ordering::SeqCst) > 0 {
                        retries_left.fetch_sub(1, Ordering::SeqCst);
                        Err(BodiesClientError::Timeout { header_hash })
                    } else {
                        Ok(BlockBody { transactions: vec![], ommers: vec![] })
                    }
                }
            })))
            .with_retries(0);

        assert_matches!(
            downloader.bodies_stream(&[(0, H256::zero())]).next().await,
            Some(Err(DownloadError::Timeout { header_hash })) => {
                assert_eq!(header_hash, H256::zero())
            }
        );
        assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 2);
    }

    mod test_utils {
        use async_trait::async_trait;
        use reth_eth_wire::BlockBody;
        use reth_interfaces::{
            p2p::bodies::{client::BodiesClient, error::BodiesClientError},
            test_utils::generators::random_block_range,
        };
        use reth_primitives::{BlockNumber, H256};
        use std::{
            collections::HashMap,
            fmt::{Debug, Formatter},
            future::Future,
            sync::Arc,
        };
        use tokio::sync::Mutex;

        /// Generate a set of bodies and their corresponding block hashes
        pub(crate) fn generate_bodies(
            rng: std::ops::Range<u64>,
        ) -> (Vec<(BlockNumber, H256)>, HashMap<H256, BlockBody>) {
            let blocks = random_block_range(rng, H256::zero());

            let hashes: Vec<(BlockNumber, H256)> =
                blocks.iter().map(|block| (block.number, block.hash())).collect();
            let bodies: HashMap<H256, BlockBody> = blocks
                .into_iter()
                .map(|block| {
                    (
                        block.hash(),
                        BlockBody {
                            transactions: block.body,
                            ommers: block
                                .ommers
                                .into_iter()
                                .map(|header| header.unseal())
                                .collect(),
                        },
                    )
                })
                .collect();

            (hashes, bodies)
        }

        /// A [BodiesClient] for testing.
        pub(crate) struct TestClient<F, Fut>(pub(crate) Arc<Mutex<F>>)
        where
            F: FnMut(H256) -> Fut + Send + Sync,
            Fut: Future<Output = Result<BlockBody, BodiesClientError>> + Send;

        impl<F, Fut> Debug for TestClient<F, Fut>
        where
            F: FnMut(H256) -> Fut + Send + Sync,
            Fut: Future<Output = Result<BlockBody, BodiesClientError>> + Send,
        {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("TestClient").finish()
            }
        }

        impl<F, Fut> TestClient<F, Fut>
        where
            F: FnMut(H256) -> Fut + Send + Sync,
            Fut: Future<Output = Result<BlockBody, BodiesClientError>> + Send,
        {
            pub(crate) fn new(f: F) -> Self {
                Self(Arc::new(Mutex::new(f)))
            }
        }

        #[async_trait]
        impl<F, Fut> BodiesClient for TestClient<F, Fut>
        where
            F: FnMut(H256) -> Fut + Send + Sync,
            Fut: Future<Output = Result<BlockBody, BodiesClientError>> + Send,
        {
            async fn get_block_body(&self, hash: H256) -> Result<BlockBody, BodiesClientError> {
                let f = &mut *self.0.lock().await;
                (f)(hash).await
            }
        }
    }
}
