use alloy_primitives::Sealable;
use futures::Stream;
use reth_network_p2p::headers::{
    downloader::{HeaderDownloader, SyncTarget},
    error::HeadersDownloaderError,
};
use reth_primitives_traits::SealedHeader;
use std::fmt::Debug;

/// A [`HeaderDownloader`] implementation that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopHeaderDownloader<H>(std::marker::PhantomData<H>);

impl<H: Sealable + Debug + Send + Sync + Unpin + 'static> HeaderDownloader
    for NoopHeaderDownloader<H>
{
    type Header = H;

    fn update_local_head(&mut self, _: SealedHeader<H>) {}

    fn update_sync_target(&mut self, _: SyncTarget) {}

    fn set_batch_size(&mut self, _: usize) {}
}

impl<H: Sealable> Stream for NoopHeaderDownloader<H> {
    type Item = Result<Vec<SealedHeader<H>>, HeadersDownloaderError<H>>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("NoopHeaderDownloader shouldn't be polled.")
    }
}
