use super::response::BlockResponse;
use crate::error::DownloadResult;
use futures::Stream;
use reth_eth_wire_types::{NetworkTypes, PrimitiveNetworkTypes};
use reth_primitives::BlockNumber;
use std::ops::RangeInclusive;

/// Body downloader return type.
pub type BodyDownloaderResult<T> = DownloadResult<Vec<BlockResponse<T>>>;

/// A downloader capable of fetching and yielding block bodies from block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [`BodiesClient`][crate::bodies::client::BodiesClient] represents a client capable of
/// fulfilling these requests.
pub trait BodyDownloader<T: NetworkTypes = PrimitiveNetworkTypes>:
    Send + Sync + Stream<Item = BodyDownloaderResult<T>> + Unpin
{
    /// Method for setting the download range.
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()>;
}
