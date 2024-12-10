use alloy_primitives::BlockNumber;
use futures::Stream;
use reth_network_p2p::{
    bodies::{downloader::BodyDownloader, response::BlockResponse},
    error::{DownloadError, DownloadResult},
};
use std::{fmt::Debug, ops::RangeInclusive};

/// A [`BodyDownloader`] implementation that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopBodiesDownloader<H, B> {
    _header: std::marker::PhantomData<H>,
    _body: std::marker::PhantomData<B>,
}

impl<H: Debug + Send + Sync + Unpin + 'static, B: Debug + Send + Sync + Unpin + 'static>
    BodyDownloader for NoopBodiesDownloader<H, B>
{
    type Body = B;
    type Header = H;

    fn set_download_range(&mut self, _: RangeInclusive<BlockNumber>) -> DownloadResult<()> {
        Ok(())
    }
}

impl<H, B> Stream for NoopBodiesDownloader<H, B> {
    type Item = Result<Vec<BlockResponse<H, B>>, DownloadError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("NoopBodiesDownloader shouldn't be polled.")
    }
}
