//! Keeps track of the state of the network.

use crate::NodeId;
use fnv::FnvHashMap;
use reth_primitives::{H256, U256};

/// Maintains the state of all peers in the network.
pub struct NetworkState {
    /// All connected peers and their state.
    peers: FnvHashMap<NodeId, PeerInfo>,
    // TODO add discovery state
}

impl NetworkState {
    // TODO add functions for adding/removing peers

    /// Propagates Block to peers.
    pub fn announce_block(&mut self, _hash: H256, _block: ()) {
        for (_id, ref mut _peer) in self.peers.iter_mut() {}
    }
}

/// Tracks the state of a Peer.
///
/// For example known blocks,so we can decide what to announce.
pub struct PeerInfo {
    /// Best block of the peer.
    pub best_hash: H256,
    /// Best block number of the peer.
    pub best_number: U256,
}

/// Tracks the current state of the peer session
pub enum PeerSessionState {}
