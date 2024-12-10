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
pub struct NoopBodiesDownloader<B>(std::marker::PhantomData<B>);

impl<B: Debug + Send + Sync + Unpin + 'static> BodyDownloader for NoopBodiesDownloader<B> {
    type Body = B;

    fn set_download_range(&mut self, _: RangeInclusive<BlockNumber>) -> DownloadResult<()> {
        Ok(())
    }
}

impl<B> Stream for NoopBodiesDownloader<B> {
    type Item = Result<Vec<BlockResponse<alloy_consensus::Header, B>>, DownloadError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("NoopBodiesDownloader shouldn't be polled.")
    }
}
