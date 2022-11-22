use super::client::BodiesClient;
use crate::p2p::bodies::error::DownloadError;
use reth_eth_wire::BlockBody;
use reth_primitives::{BlockNumber, H256};
use std::pin::Pin;
use tokio_stream::Stream;

/// A downloader capable of fetching block bodies from header hashes.
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [BodiesClient] represents a client capable of fulfilling these requests.
pub trait BodyDownloader: Sync + Send {
    /// The [BodiesClient] used to fetch the block bodies
    type Client: BodiesClient;

    /// The block bodies client
    fn client(&self) -> &Self::Client;

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
    fn bodies_stream<'a, 'b, I>(&'a self, headers: I) -> BodiesStream<'a>
    where
        I: IntoIterator<Item = &'b (BlockNumber, H256)>,
        <I as IntoIterator>::IntoIter: Send + 'b,
        'b: 'a;
}

/// A stream of block bodies.
pub type BodiesStream<'a> =
    Pin<Box<dyn Stream<Item = Result<(BlockNumber, H256, BlockBody), DownloadError>> + Send + 'a>>;
