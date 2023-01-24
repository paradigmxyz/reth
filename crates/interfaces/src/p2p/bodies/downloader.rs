use super::response::BlockResponse;
use crate::db;
use futures::Stream;
use reth_primitives::BlockNumber;
use std::ops::Range;

/// A downloader capable of fetching and yielding block bodies from block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [BodiesClient] represents a client capable of fulfilling these requests.
pub trait BodyDownloader: Send + Sync + Stream<Item = Vec<BlockResponse>> + Unpin {
    /// Method for setting the download range.
    fn set_download_range(&mut self, range: Range<BlockNumber>) -> Result<(), db::Error>;
}
