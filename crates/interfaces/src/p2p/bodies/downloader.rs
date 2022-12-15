use crate::p2p::downloader::{DownloadStream, Downloader};
use reth_primitives::{BlockLocked, SealedHeader};

/// A downloader capable of fetching block bodies from header hashes.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [BodiesClient] represents a client capable of fulfilling these requests.
pub trait BodyDownloader: Downloader {
    /// Download the bodies from `starting_block` (inclusive) up until `target_block` (inclusive).
    ///
    /// The returned stream will always emit bodies in the order they were requested, but multiple
    /// requests may be in flight at the same time.
    ///
    /// The stream may exit early in some cases. Thus, a downloader can only at a minimum guarantee:
    ///
    /// - All emitted bodies map onto a request
    /// - The emitted bodies are emitted in order: i.e. the body for the first block is emitted
    ///   first, even if it was not fetched first.
    ///
    /// It is *not* guaranteed that all the requested bodies are fetched: the downloader may close
    /// the stream before the entire range has been fetched for any reason
    fn bodies_stream<'a, 'b, I>(&'a self, headers: I) -> DownloadStream<'a, BlockLocked>
    where
        I: IntoIterator<Item = &'b SealedHeader>,
        <I as IntoIterator>::IntoIter: Send + 'b,
        'b: 'a;
}
