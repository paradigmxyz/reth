//! Commands sent to peer manager.

use std::net::SocketAddr;

use reth_network_peers::{NodeRecord, PeerId};
use tokio::sync::oneshot;

use crate::{Peer, ReputationChangeKind};

/// Commands the `PeersManager` listens for.
#[derive(Debug)]
pub enum PeerCommand {
    /// Command for manually add
    Add(PeerId, SocketAddr),
    /// Remove a peer from the set
    ///
    /// If currently connected this will disconnect the session
    Remove(PeerId),
    /// Apply a reputation change to the given peer.
    ReputationChange(PeerId, ReputationChangeKind),
    /// Get information about a peer
    GetPeer(PeerId, oneshot::Sender<Option<Peer>>),
    /// Get node information on all peers
    GetPeers(oneshot::Sender<Vec<NodeRecord>>),
}
