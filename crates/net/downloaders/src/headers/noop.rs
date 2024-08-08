use futures::Stream;
use reth_eth_wire_types::types::BlockHeader;
use reth_network_p2p::headers::{
    downloader::{HeaderDownloader, SyncTarget},
    error::HeadersDownloaderError,
};
use reth_primitives::{alloy_primitives::Sealed, Header};

/// A [`HeaderDownloader`] implementation that does nothing.
#[derive(Debug)]
#[non_exhaustive]
pub struct NoopHeaderDownloader<H = Header> {
    _phantom: std::marker::PhantomData<H>,
}

impl<H> NoopHeaderDownloader<H> {
    pub fn new() -> Self {
        Self { _phantom: Default::default() }
    }
}

impl Default for NoopHeaderDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: BlockHeader> HeaderDownloader for NoopHeaderDownloader<H> {
    type Header = H;

    fn update_local_head(&mut self, _: Sealed<H>) {}

    fn update_sync_target(&mut self, _: SyncTarget) {}

    fn set_batch_size(&mut self, _: usize) {}
}

impl<H> Stream for NoopHeaderDownloader<H> {
    type Item = Result<Vec<Sealed<H>>, HeadersDownloaderError<H>>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("NoopHeaderDownloader shouldn't be polled.")
    }
}
