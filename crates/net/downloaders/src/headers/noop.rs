use futures::Stream;
use reth_interfaces::p2p::headers::{
    downloader::{HeaderDownloader, SyncTarget},
    error::HeadersDownloaderError,
};
use reth_primitives::SealedHeader;

/// A [HeaderDownloader] implementation that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopHeaderDownloader;

impl HeaderDownloader for NoopHeaderDownloader {
    fn update_local_head(&mut self, _: SealedHeader) {}

    fn update_sync_target(&mut self, _: SyncTarget) {}

    fn set_batch_size(&mut self, _: usize) {}
}

impl Stream for NoopHeaderDownloader {
    type Item = Result<Vec<SealedHeader>, HeadersDownloaderError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("NoopHeaderDownloader shouldn't be polled.")
    }
}
