//! Discovery support for the network.

use crate::NodeId;
use futures::StreamExt;
use reth_discv4::{Discv4, NodeRecord, TableUpdate};
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::SocketAddr,
    task::{Context, Poll},
};
use tokio_stream::wrappers::ReceiverStream;

/// An abstraction over the configured discovery protocol.
///
/// Listens for new discovered nodes and emits events for discovered nodes and their address.
pub struct Discovery {
    /// List of nodes that we know exist. This is includes boot nodes or any other preconfigured
    /// nodes.
    static_addresses: HashMap<NodeId, SocketAddr>,
    /// All nodes discovered via discovery protocol.
    ///
    /// These nodes can be ephemeral and are updated via the discovery protocol.
    discovered_nodes: HashMap<NodeId, SocketAddr>,
    /// Handler to interact with the Discovery v4 service
    discv4: Discv4,
    /// All updates from the discv4 service.
    discv4_updates: ReceiverStream<TableUpdate>,
    /// Buffered events until polled.
    queued_events: VecDeque<DiscoveryEvent>,
}

impl Discovery {
    /// Manually adds an address to the set.
    pub(crate) fn add_known_address(&mut self, node_id: NodeId, addr: SocketAddr) {
        // TODO add message via disv4 to announce new node
        self.on_discv4_update(TableUpdate::Added(NodeRecord {
            address: addr.ip(),
            tcp_port: addr.port(),
            udp_port: addr.port(),
            id: node_id,
        }))
    }

    /// Returns all nodes we know exist in the network.
    pub fn known_nodes(&mut self) -> &HashMap<NodeId, SocketAddr> {
        &self.discovered_nodes
    }

    fn on_discv4_update(&mut self, update: TableUpdate) {
        match update {
            TableUpdate::Added(node) => {
                let id = node.id;
                let addr = node.tcp_addr();
                match self.discovered_nodes.entry(id) {
                    Entry::Occupied(_entry) => {}
                    Entry::Vacant(entry) => {
                        entry.insert(addr);
                        self.queued_events.push_back(DiscoveryEvent::Discovered(id, addr))
                    }
                }
            }
            TableUpdate::Removed(node) => {
                self.discovered_nodes.remove(&node);
            }
            TableUpdate::Batch(updates) => {
                for update in updates {
                    self.on_discv4_update(update);
                }
            }
        }
    }

    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DiscoveryEvent> {
        loop {
            // Drain all buffered events first
            if let Some(event) = self.queued_events.pop_front() {
                return Poll::Ready(event)
            }

            while let Poll::Ready(Some(update)) = self.discv4_updates.poll_next_unpin(cx) {
                self.on_discv4_update(update)
            }

            if self.queued_events.is_empty() {
                return Poll::Pending
            }
        }
        // drain the update stream
    }
}

/// Events produced by the [`Discovery`] manager.
pub enum DiscoveryEvent {
    /// A new node was discovered
    Discovered(NodeId, SocketAddr),
}
