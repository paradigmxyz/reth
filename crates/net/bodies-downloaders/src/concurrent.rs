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

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore]
    async fn emits_bodies_in_order() {}

    #[tokio::test]
    #[ignore]
    async fn header_iter_failure() {}

    #[tokio::test]
    #[ignore]
    async fn client_failure() {}
}
