use reth_rpc_types::PeerId;
use tokio::sync::mpsc::Receiver;

/// Consensus layer interface
#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait ClayerConsensus: Send + Sync + Clone {
    /// Returns pending consensus listener
    fn pending_consensus_listener(&self) -> Receiver<(Vec<PeerId>, reth_primitives::Bytes)>;
    /// push data received from network into cache
    fn push_received_cache(&self, peer_id: PeerId, data: reth_primitives::Bytes);
    /// pop data received from network out cache
    fn pop_received_cache(&self) -> Option<(PeerId, reth_primitives::Bytes)>;
    /// push network event(PeerConnected, PeerDisconnected)
    fn push_network_event(&self, peer_id: PeerId, connect: bool);
    /// pop network event(PeerConnected, PeerDisconnected)
    fn pop_network_event(&self) -> Option<(PeerId, bool)>;
    /// broadcast consensus
    fn broadcast_consensus(&self, peers: Vec<PeerId>, data: reth_primitives::Bytes);
}
