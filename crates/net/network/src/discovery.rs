//! Discovery support for the network.

use crate::error::NetworkError;
use futures::StreamExt;
use reth_discv4::{Discv4, Discv4Config, NodeRecord, TableUpdate};
use reth_primitives::PeerId;
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::SocketAddr,
    task::{Context, Poll},
};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;

/// An abstraction over the configured discovery protocol.
///
/// Listens for new discovered nodes and emits events for discovered nodes and their address.
pub struct Discovery {
    /// All nodes discovered via discovery protocol.
    ///
    /// These nodes can be ephemeral and are updated via the discovery protocol.
    discovered_nodes: HashMap<PeerId, SocketAddr>,
    /// Local ENR of the discovery service.
    local_enr: NodeRecord,
    /// Handler to interact with the Discovery v4 service
    discv4: Discv4,
    /// All updates from the discv4 service.
    discv4_updates: ReceiverStream<TableUpdate>,
    /// The initial config for the discv4 service
    dsicv4_config: Discv4Config,
    /// Buffered events until polled.
    queued_events: VecDeque<DiscoveryEvent>,
    /// The handle to the spawned discv4 service
    _discv4_service: JoinHandle<()>,
}

impl Discovery {
    /// Spawns the discovery service.
    ///
    /// This will spawn the [`reth_discv4::Discv4Service`] onto a new task and establish a listener
    /// channel to receive all discovered nodes.
    pub async fn new(
        discovery_addr: SocketAddr,
        sk: SecretKey,
        dsicv4_config: Discv4Config,
    ) -> Result<Self, NetworkError> {
        let local_enr = NodeRecord::from_secret_key(discovery_addr, &sk);
        let (discv4, mut discv4_service) =
            Discv4::bind(discovery_addr, local_enr, sk, dsicv4_config.clone())
                .await
                .map_err(NetworkError::Discovery)?;
        let discv4_updates = discv4_service.update_stream();

        // spawn the service
        let _discv4_service = discv4_service.spawn();

        Ok(Self {
            local_enr,
            discv4,
            discv4_updates,
            dsicv4_config,
            _discv4_service,
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
        })
    }

    /// Returns the id with which the local identifies itself in the network
    pub(crate) fn local_id(&self) -> PeerId {
        self.local_enr.id
    }

    /// Manually adds an address to the set.
    pub(crate) fn add_known_address(&mut self, peer_id: PeerId, addr: SocketAddr) {
        self.on_discv4_update(TableUpdate::Added(NodeRecord {
            address: addr.ip(),
            tcp_port: addr.port(),
            udp_port: addr.port(),
            id: peer_id,
        }))
    }

    /// Returns all nodes we know exist in the network.
    pub fn known_nodes(&mut self) -> &HashMap<PeerId, SocketAddr> {
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
    Discovered(PeerId, SocketAddr),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    use secp256k1::SECP256K1;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[tokio::test(flavor = "multi_thread")]
    async fn test_discovery_setup() {
        let mut rng = thread_rng();
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);
        let discovery_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        let _discovery =
            Discovery::new(discovery_addr, secret_key, Default::default()).await.unwrap();
    }
}
