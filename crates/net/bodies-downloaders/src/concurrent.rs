use futures_util::{stream, StreamExt, TryFutureExt};
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
    /// The batch size per one request
    pub batch_size: usize,
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
        // TODO: Retry
        Box::pin(
            stream::iter(headers.into_iter().map(|(block_number, header_hash)| {
                {
                    self.client
                        .get_block_body(*header_hash)
                        .map_ok(move |body| (*block_number, *header_hash, body))
                        .map_err(|err| match err {
                            BodiesClientError::Timeout { header_hash } => {
                                DownloadError::Timeout { header_hash }
                            }
                            err => DownloadError::Client { source: err },
                        })
                }
            }))
            .buffered(self.batch_size),
        )
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
