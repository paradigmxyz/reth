use crate::error::DownloadResult;
use alloy_primitives::BlockNumber;
use futures::Stream;
use reth_eth_wire_types::BlockAccessLists;
use std::ops::RangeInclusive;

/// Block Access Lists downloader return type.
pub type BlockAccessListsDownloaderResult = DownloadResult<Vec<BlockAccessLists>>;

/// A downloader capable of fetching and yielding block access lists.
///
/// A downloader represents a distinct strategy for submitting requests to download block access
/// lists,
/// while a [`BlockAccessListsClient`][crate::block_access_lists::client::BlockAccessListsClient]
/// represents a client capable of fulfilling these requests.
pub trait BlockAccessListsDownloader:
    Send + Stream<Item = BlockAccessListsDownloaderResult> + Unpin
{
    /// Method for setting the download range.
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()>;
}
