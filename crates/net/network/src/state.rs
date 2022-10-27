//! Keeps track of the state of the network.

use crate::NodeId;
use fnv::FnvHashMap;
use reth_primitives::{H256, U256};

/// Maintains the state of all peers in the network.
pub struct NetworkState {
    /// All connected peers and their state.
    peers: FnvHashMap<NodeId, PeerInfo>,
}

impl NetworkState {
    // TODO add functions for adding/removing peers
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
