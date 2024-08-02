//! Async peer handle.

use std::net::SocketAddr;

use derive_more::Constructor;
use reth_network_peers::{NodeRecord, PeerId};
use tokio::sync::{mpsc, oneshot};

use crate::{Peer, PeerCommand, ReputationChangeKind};

/// A communication channel to the `PeersManager` to apply manual changes to the peer set.
#[derive(Clone, Debug, Constructor)]
pub struct PeersHandle {
    /// Sender half of command channel back to the `PeersManager`
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
}

// === impl PeersHandle ===

impl PeersHandle {
    fn send(&self, cmd: PeerCommand) {
        let _ = self.manager_tx.send(cmd);
    }

    /// Adds a peer to the set.
    pub fn add_peer(&self, peer_id: PeerId, addr: SocketAddr) {
        self.send(PeerCommand::Add(peer_id, addr));
    }

    /// Removes a peer from the set.
    pub fn remove_peer(&self, peer_id: PeerId) {
        self.send(PeerCommand::Remove(peer_id));
    }

    /// Send a reputation change for the given peer.
    pub fn reputation_change(&self, peer_id: PeerId, kind: ReputationChangeKind) {
        self.send(PeerCommand::ReputationChange(peer_id, kind));
    }

    /// Returns a peer by its [`PeerId`], or `None` if the peer is not in the peer set.
    pub async fn peer_by_id(&self, peer_id: PeerId) -> Option<Peer> {
        let (tx, rx) = oneshot::channel();
        self.send(PeerCommand::GetPeer(peer_id, tx));

        rx.await.unwrap_or(None)
    }

    /// Returns all peers in the peerset.
    pub async fn all_peers(&self) -> Vec<NodeRecord> {
        let (tx, rx) = oneshot::channel();
        self.send(PeerCommand::GetPeers(tx));

        rx.await.unwrap_or_default()
    }
}
