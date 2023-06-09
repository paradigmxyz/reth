//! Discovery support for the network.

use crate::error::{NetworkError, ServiceKind};
use futures::StreamExt;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_dns_discovery::{
    DnsDiscoveryConfig, DnsDiscoveryHandle, DnsDiscoveryService, DnsNodeRecordUpdate, DnsResolver,
};
use reth_primitives::{ForkId, NodeRecord, PeerId};
use secp256k1::SecretKey;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::Arc,
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
    discovered_nodes: HashMap<PeerId, DiscoveryEntry>,
    /// Local ENR of the discovery service.
    local_enr: NodeRecord,
    /// Handler to interact with the Discovery v4 service
    discv4: Option<Discv4>,
    /// All KAD table updates from the discv4 service.
    discv4_updates: Option<ReceiverStream<DiscoveryUpdate>>,
    /// Heartbeat to periodically check for expired bond durations.
    heartbeat: tokio::time::Interval,
    /// The duration of a bond, after which a node should be checked for liveness.
    bond_duration: std::time::Duration,
    /// The handle to the spawned discv4 service
    _discv4_service: Option<JoinHandle<()>>,
    /// Handler to interact with the DNS discovery service
    _dns_discovery: Option<DnsDiscoveryHandle>,
    /// Updates from the DNS discovery service.
    dns_discovery_updates: Option<ReceiverStream<DnsNodeRecordUpdate>>,
    /// The handle to the spawned DNS discovery service
    _dns_disc_service: Option<JoinHandle<()>>,
    /// Events buffered until polled.
    queued_events: VecDeque<DiscoveryEvent>,
}

pub struct DiscoveryEntry {
    remote_addr: SocketAddr,
    fork_id: Option<ForkId>,
    last_updated_at: std::time::Instant,
}

impl DiscoveryEntry {
    pub fn update_fork_id(&mut self, fork_id: ForkId) {
        self.fork_id = Some(fork_id);
    }

    pub fn update_addr(&mut self, addr: SocketAddr) {
        self.remote_addr = addr;
    }

    pub fn reset_last_updated_at(&mut self) {
        self.last_updated_at = std::time::Instant::now();
    }
}

impl Discovery {
    /// Spawns the discovery service.
    ///
    /// This will spawn the [`reth_discv4::Discv4Service`] onto a new task and establish a listener
    /// channel to receive all discovered nodes.
    pub async fn new(
        discovery_addr: SocketAddr,
        sk: SecretKey,
        discv4_config: Option<Discv4Config>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        // setup discv4
        let local_enr = NodeRecord::from_secret_key(discovery_addr, &sk);
        let (discv4, discv4_updates, _discv4_service) = if let Some(disc_config) = discv4_config {
            let (discv4, mut discv4_service) =
                Discv4::bind(discovery_addr, local_enr, sk, disc_config).await.map_err(|err| {
                    NetworkError::from_io_error(err, ServiceKind::Discovery(discovery_addr))
                })?;
            let discv4_updates = discv4_service.update_stream();
            // spawn the service
            let _discv4_service = discv4_service.spawn();
            (Some(discv4), Some(discv4_updates), Some(_discv4_service))
        } else {
            (None, None, None)
        };

        // setup DNS discovery
        let (_dns_discovery, dns_discovery_updates, _dns_disc_service) =
            if let Some(dns_config) = dns_discovery_config {
                let (mut service, dns_disc) = DnsDiscoveryService::new_pair(
                    Arc::new(DnsResolver::from_system_conf()?),
                    dns_config,
                );
                let dns_discovery_updates = service.node_record_stream();
                let dns_disc_service = service.spawn();
                (Some(dns_disc), Some(dns_discovery_updates), Some(dns_disc_service))
            } else {
                (None, None, None)
            };

        Ok(Self {
            local_enr,
            discv4,
            discv4_updates,
            _discv4_service,
            heartbeat: tokio::time::interval(std::time::Duration::from_secs(60)),
            bond_duration: std::time::Duration::from_secs(60 * 60 * 24),
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
            _dns_disc_service,
            _dns_discovery,
            dns_discovery_updates,
        })
    }

    /// Updates the `eth:ForkId` field in discv4.
    pub(crate) fn update_fork_id(&self, fork_id: ForkId) {
        if let Some(discv4) = &self.discv4 {
            discv4.set_eip868_rlp("eth".as_bytes().to_vec(), fork_id)
        }
    }

    /// Bans the [`IpAddr`] in the discovery service.
    pub(crate) fn ban_ip(&self, ip: IpAddr) {
        if let Some(discv4) = &self.discv4 {
            discv4.ban_ip(ip)
        }
    }

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    pub(crate) fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        if let Some(discv4) = &self.discv4 {
            discv4.ban(peer_id, ip)
        }
    }

    /// Returns the id with which the local identifies itself in the network
    pub(crate) fn local_id(&self) -> PeerId {
        self.local_enr.id
    }

    /// Add a node to the discv4 table.
    pub(crate) fn add_discv4_node(&self, node: NodeRecord) {
        if let Some(discv4) = &self.discv4 {
            discv4.add_node(node);
        }
    }

    /// Processes an incoming [NodeRecord] update from a discovery service. This will update the
    /// `last_updated_at` field of the entry.
    fn on_node_record_update(&mut self, record: NodeRecord, fork_id: Option<ForkId>) {
        let id = record.id;
        let addr = record.tcp_addr();
        match self.discovered_nodes.entry(id) {
            Entry::Occupied(entry) => {
                let disc_entry = entry.into_mut();
                disc_entry.reset_last_updated_at();
                disc_entry.update_addr(addr);
                if let Some(id) = fork_id {
                    disc_entry.update_fork_id(id);
                }
            }
            Entry::Vacant(entry) => {
                let now = std::time::Instant::now();
                entry.insert(DiscoveryEntry { remote_addr: addr, fork_id, last_updated_at: now });
                self.queued_events.push_back(DiscoveryEvent::Discovered {
                    peer_id: id,
                    socket_addr: addr,
                    fork_id,
                });
            }
        }
    }

    fn on_discv4_update(&mut self, update: DiscoveryUpdate) {
        match update {
            DiscoveryUpdate::Added(record) => {
                self.on_node_record_update(record, None);
            }
            DiscoveryUpdate::EnrForkId(node, fork_id) => {
                // Also update the existing discovery entry with the `ForkId` here.
                self.on_node_record_update(node, Some(fork_id));
                self.queued_events.push_back(DiscoveryEvent::EnrForkId(node.id, fork_id))
            }
            DiscoveryUpdate::Removed(_node) => {
                // Since we want to keep track of all nodes we have discovered here,
                // we don't remove them when they are removed from the discv4 table.
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

    fn on_heartbeat(&self) {
        for (peer_id, entry) in &self.discovered_nodes {
            if entry.last_updated_at.elapsed() > self.bond_duration {
                todo!("Poll peer for liveness");
            }
        }
    }

    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DiscoveryEvent> {
        loop {
            // Drain all buffered events first
            if let Some(event) = self.queued_events.pop_front() {
                return Poll::Ready(event)
            }

            // drain the update streams
            while let Some(Poll::Ready(Some(update))) =
                self.discv4_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
            {
                self.on_discv4_update(update)
            }

            while let Some(Poll::Ready(Some(update))) =
                self.dns_discovery_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
            {
                self.add_discv4_node(update.node_record);
                self.on_node_record_update(update.node_record, update.fork_id);
            }

            if self.heartbeat.poll_tick(cx).is_ready() {
                self.on_heartbeat();
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
        Self {
            discovered_nodes: Default::default(),
            local_enr: NodeRecord {
                address: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                tcp_port: 0,
                udp_port: 0,
                id: PeerId::random(),
            },
            discv4: Default::default(),
            discv4_updates: Default::default(),
            queued_events: Default::default(),
            heartbeat: tokio::time::interval(std::time::Duration::from_secs(60)),
            bond_duration: std::time::Duration::from_secs(60 * 60 * 24),
            _discv4_service: Default::default(),
            _dns_discovery: None,
            dns_discovery_updates: None,
            _dns_disc_service: None,
        }
    }
}

/// Events produced by the [`Discovery`] manager.
pub enum DiscoveryEvent {
    /// A new node was discovered
    Discovered { peer_id: PeerId, socket_addr: SocketAddr, fork_id: Option<ForkId> },
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
            Discovery::new(discovery_addr, secret_key, Default::default(), Default::default())
                .await
                .unwrap();
    }
}
