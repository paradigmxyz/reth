//! Discovery support for the network.

use crate::{
    error::{NetworkError, ServiceKind},
    manager::DiscoveredEvent,
    StreamDiscv5,
};
use discv5;
use futures::StreamExt;
use parking_lot::RwLock;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config, EnrForkIdEntry, HandleDiscovery};
use reth_discv5::{self, DiscoveryUpdateV5, Discv5, MergedUpdateStream};
use reth_dns_discovery::{
    DnsDiscoveryConfig, DnsDiscoveryHandle, DnsDiscoveryService, DnsNodeRecordUpdate, DnsResolver,
};
use reth_primitives::{ForkId, NodeRecord, PeerId};
use secp256k1::{PublicKey, SecretKey};
use smallvec::{smallvec, SmallVec};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::info;

use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    fmt,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// An abstraction over the configured discovery protocol.
///
/// Listens for new discovered nodes and emits events for discovered nodes and their
/// address.
pub struct Discovery<D = Discv4, S = ReceiverStream<DiscoveryUpdate>> {
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
    _dns_discovery: Option<DnsDiscoveryHandle>,
    /// Updates from the DNS discovery service.
    dns_discovery_updates: Option<ReceiverStream<DnsNodeRecordUpdate>>,
    /// The handle to the spawned DNS discovery service
    _dns_disc_service: Option<JoinHandle<()>>,
    /// Events buffered until polled.
    queued_events: VecDeque<DiscoveryEvent>,
    /// List of listeners subscribed to discovery events.
    discovery_listeners: Vec<mpsc::UnboundedSender<DiscoveryEvent>>,
}

impl<D, S> Discovery<D, S>
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
            disc.set_eip868_rlp("eth".as_bytes().to_vec(), EnrForkIdEntry::from(fork_id))
        }
    }

    /// Bans the [`IpAddr`] in the discovery service.
    pub(crate) fn ban_ip(&self, ip: IpAddr) {
        if let Some(disc) = &self.disc {
            disc.ban_ip(ip)
        }
    }

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    pub(crate) fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        if let Some(disc) = &self.disc {
            disc.ban(peer_id, ip)
        }
    }

    /// Returns the id with which the local identifies itself in the network
    pub(crate) fn local_id(&self) -> PeerId {
        self.local_enr.id
    }

    /// Add a node to the discv4 table.
    pub(crate) fn add_discv4_node(&self, node: NodeRecord) {
        if let Some(disc) = &self.disc {
            disc.add_node(node);
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
                new_dns(dns_config)?
            } else {
                (None, None, None)
            };

        Ok(Self {
            discovery_listeners: Default::default(),
            local_enr,
            disc: discv4,
            disc_updates: discv4_updates,
            _disc_service: _discv4_service,
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
            _dns_disc_service,
            _dns_discovery,
            dns_discovery_updates,
        })
    }
}

impl Stream for Discovery {
    type Item = DiscoveryEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Drain all buffered events first
        if let Some(event) = self.queued_events.pop_front() {
            self.notify_listeners(&event);
            return Poll::Ready(Some(event))
        }

        // drain the update streams
        while let Some(Poll::Ready(Some(update))) =
            self.disc_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
        {
            self.on_discv4_update(update);
        }

        while let Some(Poll::Ready(Some(update))) =
            self.dns_discovery_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
        {
            self.add_discv4_node(update.node_record);
            self.on_node_record_update(update.node_record, update.fork_id);
        }

        Poll::Pending
    }
}

impl fmt::Debug for Discovery {
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

impl<S> Discovery<Discv5, S> {
    fn on_discv5_update(&mut self, update: discv5::Discv5Event) -> Result<(), NetworkError> {
        use discv5::Discv5Event::*;
        match update {
            Discovered(enr) => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::DiscoveredAtCapacity(_)

                // node has been discovered as part of a query. discv5::Discv5Config sets
                // `report_discovered_peers` to true by default.

                self.try_insert_enr_into_discovered_nodes(enr)?;
            }
            EnrAdded { .. } => {
                // not used in discv5 codebase
            }
            NodeInserted { replaced, .. } => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::Removed(_)

                if let Some(node_id) = replaced {
                    let id = into_uncompressed_id(node_id).map_err(|_| {
                        NetworkError::custom_discovery(
                            "conversion from discv5 node id to pk failed",
                        )
                    })?;

                    self.discovered_nodes.remove(&id);
                };

                // notify discv4 that a node has been inserted,
                if let Some(disc) = &self.disc {
                    disc.notify_discv4_of_kbuckets_update()
                        .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?
                }
            }
            SessionEstablished(enr, _remote_socket) => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::DiscoveredAtCapacity(_)

                // node has been discovered unrelated to a query, e.g. an incoming connection to
                // discv5

                // todo: notify discv4 also for nodes that don't make it into kbuckets? e.g. nodes
                // behind symmetric nat

                self.try_insert_enr_into_discovered_nodes(enr)?;
            }
            SocketUpdated(_socket_addr) => {}
            TalkRequest(_talk_req) => {}
        }

        Ok(())
    }

    fn try_insert_enr_into_discovered_nodes(
        &mut self,
        enr: discv5::Enr,
    ) -> Result<(), NetworkError> {
        // todo: get correct ip mode for this node
        let Some(udp_socket_ipv4) = discv5::IpMode::Ip4.get_contactable_addr(&enr) else {
            return Ok(())
        };
        // todo: get port v6 with respect to ip mode of this node
        let Some(tcp_port_ipv4) = enr.tcp4() else { return Ok(()) };

        let id = into_uncompressed_id(enr.node_id()).map_err(|_| {
            NetworkError::custom_discovery("conversion from discv5 node id to pk failed")
        })?;

        let record = NodeRecord {
            address: udp_socket_ipv4.ip(),
            tcp_port: tcp_port_ipv4,
            udp_port: udp_socket_ipv4.port(),
            id,
        };

        self.on_node_record_update(record, None);

        Ok(())
    }
}

impl<S> Stream for Discovery<Discv5, S>
where
    S: StreamDiscv5,
{
    type Item = DiscoveryEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Drain all buffered events first
        if let Some(event) = self.queued_events.pop_front() {
            self.notify_listeners(&event);
            return Poll::Ready(Some(event))
        }

        // drain the update streams
        while let Some(Poll::Ready(Some(update))) =
            self.disc_updates.as_mut().map(|ref mut updates| updates.poll_next_unpin(cx))
        {
            match update {
                DiscoveryUpdateV5::V4(update) => self.on_discv4_update(update),
                DiscoveryUpdateV5::V5(update) => {
                    if let Err(_e) = self.on_discv5_update(update) {
                        todo!()
                    }
                }
            }
        }

        while let Some(Poll::Ready(Some(update))) =
            self.dns_discovery_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
        {
            self.add_discv4_node(update.node_record);
            self.on_node_record_update(update.node_record, update.fork_id);
        }

        Poll::Pending
    }
}

impl<S> fmt::Debug for Discovery<Discv5, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Discovery<Discv5>");

        debug_struct.field("discovered_nodes", &self.discovered_nodes);
        debug_struct.field("local_enr", &self.local_enr);
        debug_struct.field("disc", &self.disc);
        debug_struct.field("dns_discovery_updates", &"{ .. }");
        debug_struct.field("queued_events", &self.queued_events);
        debug_struct.field("discovery_listeners", &self.discovery_listeners);

        debug_struct.finish()
    }
}

impl Discovery<Discv5, MergedUpdateStream> {
    /// Spawns the discovery service.
    ///
    /// This will spawn [`discv5::Discv5`] and [`Discv4`] each onto their own new task and establish
    /// a merged listener channel to receive all discovered nodes.
    pub async fn new_discv5(
        discv4_addr: SocketAddr, // discv5 addr in config
        sk: SecretKey,
        (discv4_config, discv5_config): (Discv4Config, discv5::Discv5Config),
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        //
        // 1. one port per discovery node
        //
        // get the discv5 addr
        let mut discv5_addresses: SmallVec<[SocketAddr; 2]> = smallvec!();
        use discv5::ListenConfig::*;
        match discv5_config.listen_config {
            Ipv4 { ip, port } => discv5_addresses.push((ip, port).into()),
            Ipv6 { ip, port } => discv5_addresses.push((ip, port).into()),
            DualStack { ipv4, ipv4_port, ipv6, ipv6_port } => {
                discv5_addresses.push((ipv4, ipv4_port).into());
                discv5_addresses.push((ipv6, ipv6_port).into());
            }
        };

        if discv5_addresses.iter().any(|addr| addr.port() == discv4_addr.port()) {
            return Err(NetworkError::custom_discovery(&format!("discv5 port and discv4 port can't be the same, discv5_addresses: {discv5_addresses:?}, discv4_addr: {discv4_addr}")))
        }

        //
        // 2. same key for signing enr in discv4 and discv5
        //
        // make enr for discv4
        let local_enr_discv4 = NodeRecord::from_secret_key(discv4_addr, &sk);

        // make enr for discv5
        let mut sk_copy = *sk.as_ref();
        let sk_discv5_wrapper = discv5::enr::CombinedKey::secp256k1_from_bytes(&mut sk_copy)
            .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;
        let enr = {
            let mut builder = discv5::enr::EnrBuilder::new("v4");
            builder.ip(discv5_addresses[0].ip());
            builder.udp4(discv5_addresses[0].port());
            if let Some(ipv6) = discv5_addresses.get(1) {
                builder.ip(ipv6.ip());
                builder.udp4(ipv6.port());
            }
            // todo: add additional fields from config like ipv6 etc

            // enr v4 not to get confused with discv4, independent versioning enr and discovery
            builder.build(&sk_discv5_wrapper).expect("should build enr v4")
        };

        // start the two discovery nodes
        let (disc, disc_updates) = {
            //
            // 3. start discv5
            //
            let mut discv5 = discv5::Discv5::new(enr, sk_discv5_wrapper, discv5_config)
                .map_err(NetworkError::custom_discovery)?;
            discv5.start().await.map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;

            info!("Discv5 listening on {discv5_addresses:?}");

            // start discv5 updates stream
            let discv5_updates = discv5
                .event_stream()
                .await
                .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;

            //
            // 3. types needed for interfacing with discv4
            //
            // callback passed to discv4 will only takes read lock on discv5 handle! though discv5
            // will blocking wait for write lock on its kbuckets internally to apply pending
            // nodes.
            let discv5 = Arc::new(RwLock::new(discv5));
            let discv5_ref = discv5.clone();
            let kbuckets_callback = move || -> Result<HashSet<PeerId>, secp256k1::Error> {
                let keys = discv5_ref.read().table_entries_id();

                let mut discv5_kbucket_keys = HashSet::new();

                for node_id in keys {
                    let pk_compressed_bytes = node_id.raw();
                    let pk = PublicKey::from_slice(&pk_compressed_bytes)?;
                    let pk_uncompressed_bytes =
                        PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
                    discv5_kbucket_keys.insert(pk_uncompressed_bytes);
                }

                Ok(discv5_kbucket_keys)
            };
            // channel which will tell discv4 that discv5 has updated its kbuckets
            let (discv5_kbuckets_change_tx, discv5_kbuckets_change_rx) = watch::channel(());

            //
            // 4. start discv4 as discv5 fallback, maintains a mirror of discv5 kbuckets
            //
            let (discv4, mut discv4_service) = Discv4::bind_as_secondary_disc_node(
                discv4_addr,
                local_enr_discv4,
                sk,
                discv4_config,
                discv5_kbuckets_change_rx,
                kbuckets_callback,
            )
            .await
            .map_err(|err| NetworkError::from_io_error(err, ServiceKind::Discovery(discv4_addr)))?;

            // start an update stream
            let discv4_updates = discv4_service.update_stream();

            // spawn the service
            let _discv4_service = discv4_service.spawn();

            info!("Discv4 listening on {discv4_addr}");

            //
            // 5. merge both discovery nodes
            //
            // combined handle
            let disc = Discv5::new(discv5, discv4, discv5_kbuckets_change_tx);

            // combined update stream
            let disc_updates = reth_discv5::merge_discovery_streams(discv5_updates, discv4_updates);

            // discv5 and discv4 are running like usual, only that discv4 will filter out
            // nodes already connected over discv5 identified by their public key
            (Some(disc), Some(disc_updates))
        };

        // setup DNS discovery
        let (_dns_discovery, dns_discovery_updates, _dns_disc_service) =
            if let Some(dns_config) = dns_discovery_config {
                new_dns(dns_config)?
            } else {
                (None, None, None)
            };

        Ok(Discovery {
            discovery_listeners: Default::default(),
            local_enr: local_enr_discv4,
            disc,
            disc_updates,
            _disc_service: None,
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
            _dns_disc_service,
            _dns_discovery,
            dns_discovery_updates,
        })
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn new_dns(
    dns_config: DnsDiscoveryConfig,
) -> Result<
    (
        Option<DnsDiscoveryHandle>,
        Option<ReceiverStream<DnsNodeRecordUpdate>>,
        Option<JoinHandle<()>>,
    ),
    NetworkError,
> {
    let (mut service, dns_disc) =
        DnsDiscoveryService::new_pair(Arc::new(DnsResolver::from_system_conf()?), dns_config);
    let dns_discovery_updates = service.node_record_stream();
    let dns_disc_service = service.spawn();

    Ok((Some(dns_disc), Some(dns_discovery_updates), Some(dns_disc_service)))
}

pub fn into_uncompressed_id(node_id: discv5::enr::NodeId) -> Result<PeerId, secp256k1::Error> {
    let pk_compressed_bytes = node_id.raw();
    let pk = PublicKey::from_slice(&pk_compressed_bytes)?;

    Ok(PeerId::from_slice(&pk.serialize_uncompressed()[1..]))
}

#[cfg(test)]
impl<D, S> Discovery<D, S> {
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

/// Events produced by the [`Discovery`] manager.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// Discovered a node
    NewNode(DiscoveredEvent),
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
