use super::response::BlockResponse;
use crate::error::DownloadResult;
use alloy_primitives::BlockNumber;
use futures::Stream;
use std::{fmt::Debug, ops::RangeInclusive};

/// Body downloader return type.
pub type BodyDownloaderResult<H, B> = DownloadResult<Vec<BlockResponse<H, B>>>;

/// A downloader capable of fetching and yielding block bodies from block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [`BodiesClient`][crate::bodies::client::BodiesClient] represents a client capable of
/// fulfilling these requests.
pub trait BodyDownloader:
    Send + Sync + Stream<Item = BodyDownloaderResult<Self::Header, Self::Body>> + Unpin
{
    /// The type of header that is being used
    type Header: Debug + Send + Sync + Unpin + 'static;

    /// The type of the body that is being downloaded.
    type Body: Debug + Send + Sync + Unpin + 'static;

    /// Method for setting the download range.
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()>;
}
