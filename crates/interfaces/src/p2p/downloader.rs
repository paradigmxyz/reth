use super::headers::error::DownloadError;
use crate::consensus::Consensus;
use futures::Stream;
use std::pin::Pin;

/// A stream for downloading response.
pub type DownloadStream<T> = Pin<Box<dyn Stream<Item = Result<T, DownloadError>> + Send>>;

/// The generic trait for requesting and verifying data
/// over p2p network client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait Downloader: Send + Sync {
    /// The client used to fetch necessary data
    type Client;

    /// The Consensus used to verify data validity when downloading
    type Consensus: Consensus;

    /// The headers client
    fn client(&self) -> &Self::Client;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;
}
