//! Additional support for tracking nodes.

use reth_network_peers::PeerId;
use std::{collections::HashMap, net::IpAddr, time::Instant};

/// Keeps track of nodes from which we have received a `Pong` message.
#[derive(Debug, Clone, Default)]
pub(crate) struct PongTable {
    /// The nodes we have received a `Pong` from.
    nodes: HashMap<NodeKey, Instant>,
}

impl PongTable {
    /// Updates the timestamp we received a `Pong` from the given node.
    pub(crate) fn on_pong(&mut self, remote_id: PeerId, remote_ip: IpAddr) {
        // A dual-stack socket reports IPv4 peers as IPv4-mapped addresses, while records hold the
        // IPv4 address, so bonds are keyed by the canonical address.
        let key = NodeKey { remote_id, remote_ip: remote_ip.to_canonical() };
        self.nodes.insert(key, Instant::now());
    }

    /// Returns the timestamp we received a `Pong` from the given node.
    pub(crate) fn last_pong(&self, remote_id: PeerId, remote_ip: IpAddr) -> Option<Instant> {
        self.nodes.get(&NodeKey { remote_id, remote_ip: remote_ip.to_canonical() }).copied()
    }

    /// Removes all nodes from the table that have not sent a `Pong` for at least `timeout`.
    pub(crate) fn evict_expired(&mut self, now: Instant, timeout: std::time::Duration) {
        self.nodes.retain(|_, last_pong| now - *last_pong < timeout);
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) struct NodeKey {
    pub(crate) remote_id: PeerId,
    pub(crate) remote_ip: IpAddr,
}
