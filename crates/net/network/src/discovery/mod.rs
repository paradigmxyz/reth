//! Discovery support for the network.

use crate::manager::DiscoveredEvent;
use reth_discv4::{DiscoveryUpdate, Discv4, EnrForkIdEntry};
use reth_dns_discovery::DnsDiscoveryHandle;
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::{ForkId, NodeRecord, NodeRecordWithForkId, PeerId};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::wrappers::ReceiverStream;

use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    fmt,
    net::{IpAddr, SocketAddr},
};

pub mod discv4;
pub mod discv5;
pub mod discv5_downgrade_v4;

#[cfg(feature = "discv5")]
pub use discv5::DiscoveryV5;
#[cfg(feature = "discv5-downgrade-v4")]
pub use discv5_downgrade_v4::DiscoveryV5V4;

/// An abstraction over the configured discovery protocol.
///
/// Listens for new discovered nodes and emits events for discovered nodes and their
/// address.
pub struct Discovery<D = Discv4, S = ReceiverStream<DiscoveryUpdate>, N = NodeRecord> {
    /// All nodes discovered via discovery protocol.
    ///
    /// These nodes can be ephemeral and are updated via the discovery protocol.
    discovered_nodes: HashMap<PeerId, SocketAddr>,
    /// Local ENR of the discovery service.
    local_enr: NodeRecord,
    /// Handler to interact with the Discovery service
    disc: Option<D>,
    /// All KAD table updates from the discovery service.
    disc_updates: Option<S>,
    /// The handle to the spawned discv4 service
    _disc_service: Option<JoinHandle<()>>,
    /// Handler to interact with the DNS discovery service
    _dns_discovery: Option<DnsDiscoveryHandle<N>>,
    /// Updates from the DNS discovery service.
    dns_discovery_updates: Option<ReceiverStream<NodeRecordWithForkId<N>>>,
    /// The handle to the spawned DNS discovery service
    _dns_disc_service: Option<JoinHandle<()>>,
    /// Events buffered until polled.
    queued_events: VecDeque<DiscoveryEvent>,
    /// List of listeners subscribed to discovery events.
    discovery_listeners: Vec<mpsc::UnboundedSender<DiscoveryEvent>>,
}

impl<D, S, N> Discovery<D, S, N>
where
    D: HandleDiscovery,
{
    /// Registers a listener for receiving [DiscoveryEvent] updates.
    pub(crate) fn add_listener(&mut self, tx: mpsc::UnboundedSender<DiscoveryEvent>) {
        self.discovery_listeners.push(tx);
    }

    /// Notifies all registered listeners with the provided `event`.
    #[inline]
    fn notify_listeners(&mut self, event: &DiscoveryEvent) {
        self.discovery_listeners.retain_mut(|listener| listener.send(event.clone()).is_ok());
    }

    /// Updates the `eth:ForkId` field in discv4.
    pub(crate) fn update_fork_id(&self, fork_id: ForkId) {
        if let Some(disc) = &self.disc {
            // use forward-compatible forkid entry
            disc.encode_and_set_eip868_in_local_enr(
                "eth".as_bytes().to_vec(),
                EnrForkIdEntry::from(fork_id),
            )
        }
    }

    /// Bans the [`IpAddr`] in the discovery service.
    pub(crate) fn ban_ip(&self, ip: IpAddr) {
        if let Some(disc) = &self.disc {
            disc.ban_peer_by_ip(ip)
        }
    }

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    pub(crate) fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        if let Some(disc) = &self.disc {
            disc.ban_peer_by_ip_and_node_id(peer_id, ip)
        }
    }

    /// Returns the id with which the local identifies itself in the network
    pub(crate) fn local_id(&self) -> PeerId {
        self.local_enr.id
    }

    /// Add a node to the discovery routing table table.
    pub(crate) fn add_disc_node(&self, node: NodeFromExternalSource) {
        if let Some(disc) = &self.disc {
            _ = disc.add_node_to_routing_table(node);
        }
    }

    /// Processes an incoming [NodeRecord] update from a discovery service
    fn on_node_record_update(&mut self, record: NodeRecord, fork_id: Option<ForkId>) {
        let id = record.id;
        let addr = record.tcp_addr();
        match self.discovered_nodes.entry(id) {
            Entry::Occupied(_entry) => {}
            Entry::Vacant(entry) => {
                entry.insert(addr);
                self.queued_events.push_back(DiscoveryEvent::NewNode(
                    DiscoveredEvent::EventQueued { peer_id: id, socket_addr: addr, fork_id },
                ));
            }
        }
    }

    fn on_discv4_update(&mut self, update: DiscoveryUpdate) {
        match update {
            DiscoveryUpdate::Added(record) => {
                self.on_node_record_update(record, None);
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
            DiscoveryUpdate::DiscoveredAtCapacity(record) => {
                self.on_node_record_update(record, None);
            }
        }
    }
}

impl<D, S, N> fmt::Debug for Discovery<D, S, N>
where
    D: fmt::Debug,
    N: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Discovery");

        debug_struct.field("discovered_nodes", &self.discovered_nodes);
        debug_struct.field("local_enr", &self.local_enr);
        debug_struct.field("disc", &self.disc);
        debug_struct.field("dns_discovery_updates", &self.dns_discovery_updates);
        debug_struct.field("queued_events", &self.queued_events);
        debug_struct.field("discovery_listeners", &self.discovery_listeners);

        debug_struct.finish()
    }
}

/// Events produced by the [`crate::Discovery`] manager.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// Discovered a node
    NewNode(DiscoveredEvent),
    /// Retrieved a [`ForkId`] from the peer via ENR request, See <https://eips.ethereum.org/EIPS/eip-868>
    EnrForkId(PeerId, ForkId),
}

#[cfg(test)]
impl<D, S, N> Discovery<D, S, N> {
    /// Returns a Discovery instance that does nothing and is intended for testing purposes.
    ///
    /// NOTE: This instance does nothing
    pub(crate) fn noop() -> Self {
        let (_discovery_listeners, _): (mpsc::UnboundedSender<DiscoveryEvent>, _) =
            mpsc::unbounded_channel();

        Self {
            discovered_nodes: Default::default(),
            local_enr: NodeRecord {
                address: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                tcp_port: 0,
                udp_port: 0,
                id: PeerId::random(),
            },
            disc: Default::default(),
            disc_updates: Default::default(),
            queued_events: Default::default(),
            _disc_service: Default::default(),
            _dns_discovery: None,
            dns_discovery_updates: None,
            _dns_disc_service: None,
            discovery_listeners: Default::default(),
        }
    }
}

#[cfg(test)]
#[cfg(not(any(feature = "discv5-downgrade-v4", feature = "discv5")))]
mod discv4_tests {
    use super::*;
    use rand::thread_rng;

    use secp256k1::SECP256K1;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[tokio::test(flavor = "multi_thread")]
    #[cfg(not(any(feature = "discv5-downgrade-v4", feature = "discv5")))]
    async fn test_discovery_setup() {
        let mut rng = thread_rng();
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rng);
        let discovery_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        let _discovery =
            Discovery::new(discovery_addr, secret_key, Default::default(), Default::default())
                .await
                .unwrap();
    }
}
