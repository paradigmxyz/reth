use super::error::DownloadResult;
use crate::consensus::Consensus;
use futures::Stream;
use reth_primitives::PeerId;
use std::{fmt::Debug, pin::Pin};

/// A stream for downloading response.
pub type DownloadStream<'a, T> = Pin<Box<dyn Stream<Item = DownloadResult<T>> + Send + 'a>>;

/// Generic download client for peer penalization
pub trait DownloadClient: Send + Sync + Debug {
    /// Penalize the peer for responding with a message
    /// that violates validation rules
    fn report_bad_message(&self, peer_id: PeerId);

    /// Returns how many peers the network is currently connected to.
    fn num_connected_peers(&self) -> usize;
}

/// The generic trait for requesting and verifying data
/// over p2p network client
pub trait Downloader: Send + Sync {
    /// The client used to fetch necessary data
    type Client: DownloadClient;

    /// The Consensus used to verify data validity when downloading
    type Consensus: Consensus;

    /// The headers client
    fn client(&self) -> &Self::Client;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;
}
