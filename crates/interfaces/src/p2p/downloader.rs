use crate::consensus::BeaconConsensus;
use futures::Stream;
use reth_primitives::PeerId;
use std::{fmt::Debug, pin::Pin};

/// A stream for downloading response.
pub type DownloadStream<'a, T, E> = Pin<Box<dyn Stream<Item = Result<T, E>> + Send + 'a>>;

/// Generic download client for peer penalization
pub trait DownloadClient: Send + Sync + Debug {
    /// Penalize the peer for responding with a message
    /// that violates validation rules
    fn penalize(&self, peer_id: PeerId);
}

/// The generic trait for requesting and verifying data
/// over p2p network client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait Downloader: Send + Sync {
    /// The client used to fetch necessary data
    type Client: DownloadClient;

    /// The Consensus used to verify data validity when downloading
    type Consensus: BeaconConsensus;

    /// The headers client
    fn client(&self) -> &Self::Client;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;
}
