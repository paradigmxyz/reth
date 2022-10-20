use crate::PeerId;
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
};

/// Maintains peer information.
#[derive(Debug)]
pub(crate) struct PeerSet {
    /// List of node IP addresses for which incoming connections should be rejected.
    banned_addresses: HashSet<IpAddr>,
    /// List of peers for which connections should be rejected.
    banned_peers: HashSet<PeerId>,
}

impl PeerSet {
    /// Returns true if the given address is banned
    pub fn is_banned_addr(&self, remote: &SocketAddr) -> bool {
        self.banned_addresses.contains(&remote.ip())
    }

    /// Returns true if the given `PeerId` is banned
    pub fn is_banned_peer(&self, peer_id: &PeerId) -> bool {
        self.banned_peers.contains(peer_id)
    }
}
