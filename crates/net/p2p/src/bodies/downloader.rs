use super::response::BlockResponse;
use crate::error::DownloadResult;
use alloy_primitives::BlockNumber;
use futures::Stream;
use reth_primitives_traits::Block;
use std::ops::RangeInclusive;

/// Body downloader return type.
pub type BodyDownloaderResult<B> = DownloadResult<Vec<BlockResponse<B>>>;

/// A downloader capable of fetching and yielding block bodies from block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [`BodiesClient`][crate::bodies::client::BodiesClient] represents a client capable of
/// fulfilling these requests.
pub trait BodyDownloader:
    Send + Sync + Stream<Item = BodyDownloaderResult<Self::Block>> + Unpin
{
    /// The Block type this downloader supports
    type Block: Block + 'static;

    /// Method for setting the download range.
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()>;
}
