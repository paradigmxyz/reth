use super::response::BlockResponse;
use crate::p2p::downloader::Downloader;
use futures::Stream;
use reth_primitives::SealedHeader;

/// A downloader capable of fetching and yielding block bodies from block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [BodiesClient] represents a client capable of fulfilling these requests.
pub trait BodyDownloader: Downloader + Stream<Item = Vec<BlockResponse>> + Unpin {
    /// Method for setting the current headers for block body download.
    fn set_headers(&mut self, headers: Vec<SealedHeader>);
}
