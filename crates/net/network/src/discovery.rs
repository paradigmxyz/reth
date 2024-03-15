//! Discovery support for the network.

use crate::{
    error::{NetworkError, ServiceKind},
    manager::DiscoveredEvent,
};
use discv5::enr::{CombinedPublicKey, Enr, EnrPublicKey};
use futures::StreamExt;
use parking_lot::RwLock;
use reth_discv4::{
    DiscoveryUpdate, Discv4, Discv4Config, EnrForkIdEntry, HandleDiscovery, NodeFromExternalSource,
    PublicKey, SecretKey,
};
use reth_discv5::{
    enr::uncompressed_id_from_enr_pk, DiscoveryUpdateV5, Discv5WithDiscv4Downgrade,
    MergedUpdateStream,
};
use reth_dns_discovery::{
    DnsDiscoveryConfig, DnsDiscoveryHandle, DnsDiscoveryService, DnsResolver, Update,
};
use reth_primitives::{ForkId, NodeRecord, NodeRecordWithForkId, PeerId};
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
                new_dns::<NodeRecord>(dns_config)?
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
            self.add_disc_node(NodeFromExternalSource::NodeRecord(update.node_record));
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

impl<S, N> Discovery<Discv5WithDiscv4Downgrade, S, N> {
    fn on_discv5_update(&mut self, update: discv5::Event) -> Result<(), NetworkError> {
        use discv5::Event::*;
        match update {
            Discovered(enr) => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::DiscoveredAtCapacity(_)

                // node has been discovered as part of a query. discv5::Config sets
                // `report_discovered_peers` to true by default.

                self.try_insert_enr_into_discovered_nodes(enr)?;
            }
            EnrAdded { .. } => {
                // not used in discv5 codebase
            }
            NodeInserted { replaced, .. } => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::Removed(_)

                // todo: store the smaller discv5 node ids and convert peer id on discv4
                // update instead
                if let Some(ref disc) = self.disc {
                    if let Some(ref node_id) = replaced {
                        if let Some(peer_id) = disc
                            .with_discv5(|discv5| {
                                discv5.with_kbuckets(|kbuckets| {
                                    kbuckets
                                        .read()
                                        .iter_ref()
                                        .find(|entry| entry.node.key.preimage() == node_id)
                                        .map(|entry| uncompressed_id_from_enr_pk(entry.node.value))
                                })
                            })
                            .transpose()
                            .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?
                        {
                            self.discovered_nodes.remove(&peer_id);
                        }
                    }
                }
            }
            SessionEstablished(enr, _remote_socket) => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::DiscoveredAtCapacity(_)

                // node has been discovered unrelated to a query, e.g. an incoming connection to
                // discv5

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

        let id = uncompressed_id_from_enr_pk(&enr).map_err(|_| {
            NetworkError::custom_discovery("failed to extract peer id from discv5 enr")
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

impl<S> Stream for Discovery<Discv5WithDiscv4Downgrade, S, Enr<SecretKey>>
where
    S: Stream<Item = DiscoveryUpdateV5> + Unpin + Send + 'static,
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
            self.add_disc_node(NodeFromExternalSource::Enr(update.node_record.clone()));
            if let Ok(node_record) = update.node_record.try_into() {
                self.on_node_record_update(node_record, update.fork_id);
            }
        }

        Poll::Pending
    }
}

impl<S, N> fmt::Debug for Discovery<Discv5WithDiscv4Downgrade, S, N>
where
    N: fmt::Debug,
{
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

impl Discovery<Discv5WithDiscv4Downgrade, MergedUpdateStream, Enr<SecretKey>> {
    /// Spawns the discovery service.
    ///
    /// This will spawn [`discv5::Discv5`] and [`Discv4`] each onto their own new task and
    /// establish a merged listener channel to receive all discovered nodes.
    ///
    /// Note: if dns discovery is configured, any nodes found by this service will be
    pub async fn start_discv5_with_discv4_downgrade(
        discv4_addr: SocketAddr, // discv5 addr in config
        sk: SecretKey,
        disc_config: (Option<Discv4Config>, Option<discv5::Config>),
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        let (disc, disc_updates, local_enr_discv4) = match disc_config {
            (Some(discv4_config), Some(discv5_config)) => {
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
                    return Err(NetworkError::custom_discovery(&format!(
                        "discv5 port and discv4 port can't be the same,
                        discv5_addresses: {discv5_addresses:?},
                        discv4_addr: {discv4_addr}"
                    )))
                }

                //
                // 2. same key for signing enr in discv4 and discv5
                //
                // make enr for discv4
                let local_enr_discv4 = NodeRecord::from_secret_key(discv4_addr, &sk);

                // make enr for discv5
                let mut sk_copy = *sk.as_ref();
                let sk_discv5_wrapper =
                    discv5::enr::CombinedKey::secp256k1_from_bytes(&mut sk_copy)
                        .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;
                let enr = {
                    let mut builder = discv5::enr::Enr::builder();
                    builder.ip(discv5_addresses[0].ip());
                    builder.udp4(discv5_addresses[0].port());
                    if let Some(ipv6) = discv5_addresses.get(1) {
                        builder.ip(ipv6.ip());
                        builder.udp4(ipv6.port());
                    }
                    // todo: add additional fields from config like ipv6 etc

                    // enr v4 not to get confused with discv4, independent versioning enr and
                    // discovery
                    builder.build(&sk_discv5_wrapper).expect("should build enr v4")
                };

                // start the two discovery nodes

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
                // callback passed to discv4 will only takes read lock on discv5 handle! though
                // discv5 will blocking wait for write lock on its kbuckets
                // internally to apply pending nodes.
                let discv5 = Arc::new(RwLock::new(discv5));
                let discv5_ref = discv5.clone();
                // todo: pass mutual ref to mirror as param to filter out removed nodes and only
                // get peer ids of additions.
                let read_kbuckets_callback =
                    move || -> Result<HashSet<PeerId>, secp256k1::Error> {
                        let keys = discv5_ref.read().with_kbuckets(|kbuckets| {
                            kbuckets
                                .read()
                                .iter_ref()
                                .map(|node| {
                                    let enr = node.node.value;
                                    let pk = enr.public_key();
                                    debug_assert!(
                                        matches!(pk, CombinedPublicKey::Secp256k1(_)),
                                        "discv5 using different key type than discv4"
                                    );
                                    pk.encode()
                                })
                                .collect::<Vec<_>>()
                        });

                        let mut discv5_kbucket_keys = HashSet::with_capacity(keys.len());

                        for pk_bytes in keys {
                            let pk = PublicKey::from_slice(&pk_bytes)?;
                            let peer_id = PeerId::from_slice(&pk.serialize_uncompressed()[1..]);
                            discv5_kbucket_keys.insert(peer_id);
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
                    read_kbuckets_callback,
                )
                .await
                .map_err(|err| {
                    NetworkError::from_io_error(err, ServiceKind::Discovery(discv4_addr))
                })?;

                // start an update stream
                let discv4_updates = discv4_service.update_stream();

                // spawn the service
                let _discv4_service = discv4_service.spawn();

                info!("Discv4 listening on {discv4_addr}");

                //
                // 5. merge both discovery nodes
                //
                // combined handle
                let disc = Discv5WithDiscv4Downgrade::new(discv5, discv4);

                // combined update stream
                let disc_updates = MergedUpdateStream::merge_discovery_streams(
                    discv5_updates,
                    discv4_updates,
                    discv5_kbuckets_change_tx,
                );

                // discv5 and discv4 are running like usual, only that discv4 will filter out
                // nodes already connected over discv5 identified by their public key
                (Some(disc), Some(disc_updates), local_enr_discv4)
            }
            _ => {
                // make enr for discv4 not to break existing api, possibly used in tests
                let local_enr_discv4 = NodeRecord::from_secret_key(discv4_addr, &sk);
                (None, None, local_enr_discv4)
            }
        };

        // setup DNS discovery.
        let (_dns_discovery, dns_discovery_updates, _dns_disc_service) =
            if let Some(dns_config) = dns_discovery_config {
                new_dns::<Enr<SecretKey>>(dns_config)?
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
pub(crate) fn new_dns<N>(
    dns_config: DnsDiscoveryConfig,
) -> Result<
    (
        Option<DnsDiscoveryHandle<N>>,
        Option<ReceiverStream<NodeRecordWithForkId<N>>>,
        Option<JoinHandle<()>>,
    ),
    NetworkError,
>
where
    N: Clone + Send + 'static,
    DnsDiscoveryService<DnsResolver, N>: Update,
    DnsDiscoveryService<DnsResolver, N>: Stream,
    <DnsDiscoveryService<DnsResolver, N> as Stream>::Item: fmt::Debug,
{
    let (mut service, dns_disc) =
        DnsDiscoveryService::new_pair(Arc::new(DnsResolver::from_system_conf()?), dns_config);
    let dns_discovery_updates = service.node_record_stream();
    let dns_disc_service = service.spawn();

    Ok((Some(dns_disc), Some(dns_discovery_updates), Some(dns_disc_service)))
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
#[cfg(not(feature = "discv5"))]
mod discv4_tests {
    use super::*;
    use rand::thread_rng;

    use reth_primitives::{Hardfork, MAINNET};

    use secp256k1::SECP256K1;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[tokio::test(flavor = "multi_thread")]
    #[cfg(not(feature = "discv5"))]
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

#[cfg(test)]
#[cfg(feature = "discv5")]
mod discv5_tests {
    use super::*;

    use reth_discv4::Discv4ConfigBuilder;
    use reth_discv5::EnrCombinedKeyWrapper;
    use tracing::trace;

    use rand::thread_rng;

    async fn start_discv5_with_discv4_downgrade_node(
        udp_port_discv4: u16,
        udp_port_discv5: u16,
    ) -> Discovery<Discv5WithDiscv4Downgrade, MergedUpdateStream, enr::Enr<secp256k1::SecretKey>>
    {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv4_addr = format!("127.0.0.1:{udp_port_discv4}").parse().unwrap();
        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        // disable `NatResolver`
        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = discv5::ConfigBuilder::new(discv5_listen_config).build();

        Discovery::start_discv5_with_discv4_downgrade(
            discv4_addr,
            secret_key,
            (Some(discv4_config), Some(discv5_config)),
            None,
        )
        .await
        .expect("should build discv5 with discv4 downgrade")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discv5_with_discv4_downgrade() {
        reth_tracing::init_test_tracing();

        let mut node_1 = start_discv5_with_discv4_downgrade_node(30314, 30315).await;
        let node_1_enr = node_1.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let node_1_enr_without_sig = node_1.local_enr;

        let mut node_2 = start_discv5_with_discv4_downgrade_node(30324, 30325).await;
        let node_2_enr = node_2.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let node_2_enr_without_sig = node_2.local_enr;

        trace!(target: "reth_network::discovery::discv5_tests",
            node_1_node_id=format!("{:#}", node_1_enr_without_sig.id),
            node_2_node_id=format!("{:#}", node_2_enr_without_sig.id),
            "started nodes"
        );

        // add node_2 manually to node_1:discv4 kbuckets
        node_1.disc.as_ref().unwrap().with_discv4(|discv4| {
            _ = discv4.add_node_to_routing_table(NodeFromExternalSource::NodeRecord(
                node_2_enr_without_sig,
            ));
        });

        // verify node_2 is in KBuckets of node_1:discv4 and vv
        let event_1_v4 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_2_v4 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Added(node)) if node == node_2_enr_without_sig);
        matches!(event_2_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Added(node)) if node == node_1_enr_without_sig);

        // add node_2 to discovery handle of node_1 (should add node to discv5 kbuckets)
        let node_2_enr_reth_compatible_ty: Enr<SecretKey> =
            EnrCombinedKeyWrapper(node_2_enr.clone()).try_into().unwrap();
        node_1
            .disc
            .as_ref()
            .unwrap()
            .add_node_to_routing_table(NodeFromExternalSource::Enr(node_2_enr_reth_compatible_ty))
            .unwrap();
        // verify node_2 is in KBuckets of node_1:discv5
        assert!(node_1
            .disc
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.table_entries_id().contains(&node_2_enr.node_id())));

        // manually trigger connection from node_1 to node_2
        node_1
            .disc
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.send_ping(node_2_enr.clone()))
            .await
            .unwrap();

        // verify node_1:discv5 is connected to node_2:discv5 and vv
        let event_2_v5 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_1_v5 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v5, DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(node, socket)) if node == node_2_enr && socket == node_2_enr.udp4_socket().unwrap().into());
        matches!(event_2_v5, DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(node, socket)) if node == node_1_enr && socket == node_1_enr.udp4_socket().unwrap().into());

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_2_v5, DiscoveryUpdateV5::V5(discv5::Event::NodeInserted { node_id, replaced }) if node_id == node_1_enr.node_id() && replaced.is_none());

        let event_2_v4 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_1_v4 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Removed(node_id)) if node_id == node_2_enr_without_sig.id);
        matches!(event_2_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Removed(node_id)) if node_id == node_1_enr_without_sig.id);
    }
}
