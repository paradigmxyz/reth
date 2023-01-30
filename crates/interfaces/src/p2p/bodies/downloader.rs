use super::response::BlockResponse;
use crate::p2p::error::DownloadResult;
use futures::Stream;
use reth_primitives::BlockNumber;
use std::ops::Range;

/// Body downloader return type.
pub type BodyDownloaderResult = DownloadResult<Vec<BlockResponse>>;

/// A downloader capable of fetching and yielding block bodies from block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [BodiesClient][crate::p2p::bodies::client::BodiesClient] represents a client capable of
/// fulfilling these requests.
pub trait BodyDownloader: Send + Sync + Stream<Item = BodyDownloaderResult> + Unpin {
    /// Method for setting the download range.
    fn set_download_range(&mut self, range: Range<BlockNumber>) -> DownloadResult<()>;
}
