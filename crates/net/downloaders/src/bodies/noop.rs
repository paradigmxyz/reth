use futures::Stream;
use reth_interfaces::p2p::{
    bodies::{downloader::BodyDownloader, response::BlockResponse},
    error::{DownloadError, DownloadResult},
};
use reth_primitives::BlockNumber;
use std::ops::RangeInclusive;

/// A [BodyDownloader] implementation that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopBodiesDownloader;

impl BodyDownloader for NoopBodiesDownloader {
    fn set_download_range(&mut self, _: RangeInclusive<BlockNumber>) -> DownloadResult<()> {
        Ok(())
    }
}

impl Stream for NoopBodiesDownloader {
    type Item = Result<Vec<BlockResponse>, DownloadError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        panic!("NoopBodiesDownloader shouldn't be polled.")
    }
}
