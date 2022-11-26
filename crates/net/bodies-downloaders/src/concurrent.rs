use backon::{ExponentialBackoff, Retryable};
use futures_util::{stream, StreamExt};
use reth_eth_wire::BlockBody;
use reth_interfaces::p2p::{
    bodies::{
        client::BodiesClient,
        downloader::{BodiesStream, BodyDownloader},
    },
    error::RequestResult,
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
    ) -> RequestResult<(BlockNumber, H256, BlockBody)> {
        let mut body = self.client.get_block_body(vec![header_hash]).await?;
        Ok((block_number, header_hash, body.remove(0)))
    }
}

// TODO: Cleanup
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        concurrent::ConcurrentDownloader,
        test_utils::{generate_bodies, TestClient},
    };
    use assert_matches::assert_matches;
    use futures_util::stream::{StreamExt, TryStreamExt};
    use reth_interfaces::p2p::{bodies::downloader::BodyDownloader, error::RequestError};
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

        let downloader = ConcurrentDownloader::new(Arc::new(TestClient::new(|hash: Vec<H256>| {
            let mut bodies = bodies.clone();
            async move {
                // Simulate that the request for this (random) block takes 0-100ms
                tokio::time::sleep(Duration::from_millis(hash[0].to_low_u64_be() % 100)).await;

                Ok(vec![bodies
                    .remove(&hash[0])
                    .expect("Downloader asked for a block it should not ask for")])
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
        let downloader =
            ConcurrentDownloader::new(Arc::new(TestClient::new(|_: Vec<H256>| async {
                Err(RequestError::ChannelClosed)
            })));

        assert_matches!(
            downloader.bodies_stream(&[(0, H256::zero())]).next().await,
            Some(Err(RequestError::ChannelClosed))
        );
    }

    /// Checks that the body request is retried on timeouts
    #[tokio::test]
    async fn retries_timeouts() {
        let retries_left = Arc::new(AtomicUsize::new(3));
        let downloader =
            ConcurrentDownloader::new(Arc::new(TestClient::new(|mut header_hash: Vec<H256>| {
                let retries_left = retries_left.clone();
                let _header_hash = header_hash.remove(0);
                async move {
                    if retries_left.load(Ordering::SeqCst) > 0 {
                        retries_left.fetch_sub(1, Ordering::SeqCst);
                        Err(RequestError::Timeout)
                    } else {
                        Ok(vec![BlockBody { transactions: vec![], ommers: vec![] }])
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
            ConcurrentDownloader::new(Arc::new(TestClient::new(|mut header_hash: Vec<H256>| {
                let _header_hash = header_hash.remove(0);
                let retries_left = retries_left.clone();
                async move {
                    if retries_left.load(Ordering::SeqCst) > 0 {
                        retries_left.fetch_sub(1, Ordering::SeqCst);
                        Err(RequestError::Timeout)
                    } else {
                        Ok(vec![BlockBody { transactions: vec![], ommers: vec![] }])
                    }
                }
            })))
            .with_retries(0);

        assert_matches!(
            downloader.bodies_stream(&[(0, H256::zero())]).next().await,
            Some(Err(RequestError::Timeout))
        );
        assert_eq!(Arc::try_unwrap(retries_left).unwrap().into_inner(), 2);
    }
}
