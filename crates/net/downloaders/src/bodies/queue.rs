use super::request::BodiesRequestFuture;
use crate::metrics::DownloaderMetrics;
use futures::{stream::FuturesUnordered, Stream};
use futures_util::StreamExt;
use reth_interfaces::{
    consensus::Consensus,
    p2p::{
        bodies::{client::BodiesClient, response::BlockResponse},
        error::DownloadResult,
    },
};
use reth_primitives::{BlockNumber, SealedHeader};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// The wrapper around [FuturesUnordered] that keeps information
/// about the blocks currently being requested.
#[derive(Debug)]
pub(crate) struct BodiesRequestQueue<B: BodiesClient> {
    /// Inner body request queue.
    inner: FuturesUnordered<BodiesRequestFuture<B>>,
    /// The downloader metrics.
    metrics: DownloaderMetrics,
    /// Last requested block number.
    pub(crate) last_requested_block_number: Option<BlockNumber>,
}

impl<B> BodiesRequestQueue<B>
where
    B: BodiesClient + 'static,
{
    /// Create new instance of request queue.
    pub(crate) fn new(metrics: DownloaderMetrics) -> Self {
        Self { metrics, inner: Default::default(), last_requested_block_number: None }
    }

    /// Returns `true` if the queue is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of queued requests.
    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    /// Clears the inner queue and related data.
    pub(crate) fn clear(&mut self) {
        self.inner.clear();
        self.last_requested_block_number.take();
    }

    /// Add new request to the queue.
    /// Expects a sorted list of headers.
    pub(crate) fn push_new_request(
        &mut self,
        client: Arc<B>,
        consensus: Arc<dyn Consensus>,
        request: Vec<SealedHeader>,
    ) {
        // Set last max requested block number
        self.last_requested_block_number = request
            .last()
            .map(|last| match self.last_requested_block_number {
                Some(num) => last.number.max(num),
                None => last.number,
            })
            .or(self.last_requested_block_number);
        // Create request and push into the queue.
        self.inner.push(
            BodiesRequestFuture::new(client, consensus, self.metrics.clone()).with_headers(request),
        )
    }
}

impl<B> Stream for BodiesRequestQueue<B>
where
    B: BodiesClient + 'static,
{
    type Item = DownloadResult<Vec<BlockResponse>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.poll_next_unpin(cx)
    }
}
