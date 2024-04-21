use reth_primitives::PeerId;
use std::fmt::Debug;

/// Generic download client for peer penalization
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait DownloadClient: Send + Sync + Debug {
    /// Penalize the peer for responding with a message
    /// that violates validation rules
    fn report_bad_message(&self, peer_id: PeerId);

    /// Returns how many peers the network is currently connected to.
    fn num_connected_peers(&self) -> usize;
}
