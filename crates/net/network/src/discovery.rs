//! Discovery support for the network.

use crate::error::NetworkError;
use futures::StreamExt;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_primitives::{ForkId, NodeRecord, PeerId};
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
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
    /// All KAD table updates from the discv4 service.
    discv4_updates: ReceiverStream<DiscoveryUpdate>,
    /// The initial config for the discv4 service
    _dsicv4_config: Discv4Config,
    /// Events buffered until polled.
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
            _dsicv4_config: dsicv4_config,
            _discv4_service,
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
        })
    }

    /// Updates the `eth:ForkId` field in discv4.
    #[allow(unused)]
    pub(crate) fn update_fork_id(&self, fork_id: ForkId) {
        self.discv4.set_eip868_rlp("eth".as_bytes().to_vec(), fork_id)
    }

    /// Bans the [`IpAddr`] in the discovery service.
    pub(crate) fn ban_ip(&self, ip: IpAddr) {
        self.discv4.ban_ip(ip)
    }

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    pub(crate) fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        self.discv4.ban(peer_id, ip)
    }

    /// Returns the id with which the local identifies itself in the network
    pub(crate) fn local_id(&self) -> PeerId {
        self.local_enr.id
    }

    fn on_discv4_update(&mut self, update: DiscoveryUpdate) {
        match update {
            DiscoveryUpdate::Added(node) => {
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
            DiscoveryUpdate::EnrForkId(node, fork_id) => {
                self.queued_events.push_back(DiscoveryEvent::EnrForkId(node.id, fork_id))
            }
            DiscoveryUpdate::Removed(node) => {
                self.discovered_nodes.remove(&node);
            }
            DiscoveryUpdate::Batch(updates) => {
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

            // drain the update stream
            while let Poll::Ready(Some(update)) = self.discv4_updates.poll_next_unpin(cx) {
                self.on_discv4_update(update)
            }

            if self.queued_events.is_empty() {
                return Poll::Pending
            }
        }
    }
}

#[cfg(test)]
impl Discovery {
    /// Returns a Discovery instance that does nothing and is intended for testing purposes.
    ///
    /// NOTE: This instance does nothing
    pub(crate) fn noop() -> Self {
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Self {
            discovered_nodes: Default::default(),
            local_enr: NodeRecord {
                address: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                tcp_port: 0,
                udp_port: 0,
                id: PeerId::random(),
            },
            discv4: Discv4::noop(),
            discv4_updates: ReceiverStream::new(rx),
            _dsicv4_config: Default::default(),
            queued_events: Default::default(),
            _discv4_service: tokio::task::spawn(async move {}),
        }
    }
}

/// Events produced by the [`Discovery`] manager.
pub enum DiscoveryEvent {
    /// A new node was discovered
    Discovered(PeerId, SocketAddr),
    /// Retrieved a [`ForkId`] from the peer via ENR request, See <https://eips.ethereum.org/EIPS/eip-868>
    EnrForkId(PeerId, ForkId),
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
