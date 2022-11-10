use futures_util::{stream, StreamExt, TryFutureExt};
use reth_interfaces::p2p::bodies::{
    client::BodiesClient,
    downloader::{BodiesStream, DownloadError, Downloader},
};
use reth_primitives::H256;
use std::{sync::Arc, time::Duration};

/// Downloads bodies in batches.
///
/// All blocks in a batch are fetched at the same time.
#[derive(Debug)]
pub struct ConcurrentDownloader<C> {
    /// The bodies client
    client: Arc<C>,
    /// The batch size per one request
    pub batch_size: usize,
    /// A single request timeout
    pub request_timeout: Duration,
    /// The number of retries for downloading
    pub request_retries: usize,
}

impl<C: BodiesClient> Downloader for ConcurrentDownloader<C> {
    type Client = C;

    /// The request timeout duration
    fn timeout(&self) -> Duration {
        self.request_timeout
    }

    /// The block bodies client
    fn client(&self) -> &Self::Client {
        &self.client
    }

    fn stream_bodies<'a, 'b, I>(&'a self, headers: I) -> BodiesStream<'a>
    where
        I: IntoIterator<Item = H256>,
        <I as IntoIterator>::IntoIter: Send + 'b,
        'b: 'a,
    {
        // TODO: Timeout
        Box::pin(
            stream::iter(headers.into_iter().map(|header_hash| {
                {
                    self.client
                        .get_block_body(header_hash)
                        .map_ok(move |body| (header_hash, body))
                        // TODO: Real error
                        .map_err(move |_| DownloadError::Timeout { header_hash })
                }
            }))
            .buffered(self.batch_size),
        )
    }
}

/// A [ConcurrentDownloader] builder.
#[derive(Debug)]
pub struct ConcurrentDownloaderBuilder {
    /// The batch size per one request
    batch_size: usize,
    /// A single request timeout
    request_timeout: Duration,
    /// The number of retries for downloading
    request_retries: usize,
}

impl Default for ConcurrentDownloaderBuilder {
    fn default() -> Self {
        Self { batch_size: 100, request_timeout: Duration::from_millis(100), request_retries: 5 }
    }
}

impl ConcurrentDownloaderBuilder {
    /// Initialize a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the request batch size
    pub fn batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set the request timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set the number of retries per request
    pub fn retries(mut self, retries: usize) -> Self {
        self.request_retries = retries;
        self
    }

    /// Build [ConcurrentDownloader] with the provided client
    pub fn build<C: BodiesClient>(self, client: Arc<C>) -> ConcurrentDownloader<C> {
        ConcurrentDownloader {
            client,
            batch_size: self.batch_size,
            request_timeout: self.request_timeout,
            request_retries: self.request_retries,
        }
    }
}
