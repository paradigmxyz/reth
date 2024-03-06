use reth_primitives::B256;
use reth_rpc_types::PeerId;
use tokio::sync::mpsc::Receiver;

/// Consensus layer event
#[derive(Debug)]
pub enum ClayerConsensusEvent {
    /// Peer connected or disconnected
    PeerNetWork(PeerId, bool),
    /// Consensus message
    PeerMessage(PeerId, reth_primitives::Bytes),
    /// Consensus OnBlockValid
    BlockValid(B256),
    /// Consensus OnBlockInvalid
    BlockInvalid(B256),
    /// Consensus OnBlockCommit
    BlockCommit((B256, u64, bool)),
}

/// Consensus layer interface
#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait ClayerConsensusMessageAgentTrait: Send + Sync + Clone {
    /// Returns pending consensus listener
    fn pending_consensus_listener(&self) -> Receiver<(Vec<PeerId>, reth_primitives::Bytes)>;
    /// push data received from network into cache
    fn push_received_cache(&self, peer_id: PeerId, data: reth_primitives::Bytes);

    /// push network event(PeerConnected, PeerDisconnected)
    fn push_network_event(&self, peer_id: PeerId, connect: bool);
    /// pop network event(PeerConnected, PeerDisconnected)
    fn pop_event(&self) -> Option<ClayerConsensusEvent>;

    /// push block event
    fn push_block_event(&self, event: ClayerConsensusEvent);

    /// broadcast consensus
    fn broadcast_consensus(&self, peers: Vec<PeerId>, data: reth_primitives::Bytes);
    /// get all peers
    fn get_peers(&self) -> Vec<PeerId>;
}
