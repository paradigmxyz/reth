use reth_rpc_types::PeerId;
use tokio::sync::mpsc::Receiver;

#[derive(Debug)]
pub enum ClayerConsensusEvent {
    PeerNetWork(PeerId, bool),
    PeerMessage(PeerId, reth_primitives::Bytes),
}

/// Consensus layer interface
#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait ClayerConsensusMessageAgentTrait: Send + Sync + Clone {
    /// Returns pending consensus listener
    fn pending_consensus_listener(&self) -> Receiver<(Vec<PeerId>, reth_primitives::Bytes)>;
    /// push data received from network into cache
    fn push_received_cache(&self, peer_id: PeerId, data: reth_primitives::Bytes);
    /// push data received from self into cache
    fn push_received_cache_first(&self, peer_id: PeerId, msg: reth_primitives::Bytes);
    /// push network event(PeerConnected, PeerDisconnected)
    fn push_network_event(&self, peer_id: PeerId, connect: bool);
    // /// pop network event(PeerConnected, PeerDisconnected)
    // fn pop_event(&self) -> Option<ClayerConsensusEvent>;

    /// replace pop_event
    fn receiver(&self) -> crossbeam_channel::Receiver<ClayerConsensusEvent>;

    /// broadcast consensus
    fn broadcast_consensus(&self, peers: Vec<PeerId>, data: reth_primitives::Bytes);
    /// get all peers
    fn get_peers(&self) -> Vec<PeerId>;
}
