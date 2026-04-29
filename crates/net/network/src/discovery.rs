//! Discovery support for the network.

use crate::{
    cache::LruMap,
    error::{NetworkError, ServiceKind},
};
use enr::Enr;
use futures::StreamExt;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{DiscoveredPeer, Discv5};
use reth_dns_discovery::{
    DnsDiscoveryConfig, DnsDiscoveryHandle, DnsDiscoveryService, DnsNodeRecordUpdate, DnsResolver,
};
use reth_ethereum_forks::{EnrForkIdEntry, ForkId};
use reth_network_api::{DiscoveredEvent, DiscoveryEvent};
use reth_network_peers::{NodeRecord, PeerId};
use reth_network_types::PeerAddr;
use secp256k1::SecretKey;
use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{net::UdpSocket, sync::mpsc, task::JoinHandle};
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::{debug, trace};

/// Default max capacity for cache of discovered peers.
///
/// Default is 10 000 peers.
pub const DEFAULT_MAX_CAPACITY_DISCOVERED_PEERS_CACHE: u32 = 10_000;

/// An abstraction over the configured discovery protocol.
///
/// Listens for new discovered nodes and emits events for discovered nodes and their
/// address.
#[derive(Debug)]
pub struct Discovery {
    /// All nodes discovered via discovery protocol.
    ///
    /// These nodes can be ephemeral and are updated via the discovery protocol.
    discovered_nodes: LruMap<PeerId, PeerAddr>,
    /// Local ENR of the discovery v4 service (discv5 ENR has same [`PeerId`]).
    local_enr: NodeRecord,
    /// Handler to interact with the Discovery v4 service
    discv4: Option<Discv4>,
    /// All KAD table updates from the discv4 service.
    discv4_updates: Option<ReceiverStream<DiscoveryUpdate>>,
    /// The handle to the spawned discv4 service
    _discv4_service: Option<JoinHandle<()>>,
    /// Handler to interact with the Discovery v5 service
    discv5: Option<Discv5>,
    /// All KAD table updates from the discv5 service.
    discv5_updates: Option<ReceiverStream<discv5::Event>>,
    /// Background task that, in shared-port mode, drains `UnrecognizedFrame`s from discv5 and
    /// feeds them into the discv4 ingress so packets advance without polling `Discovery`.
    _discv5_forwarder: Option<JoinHandle<()>>,
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

impl Discovery {
    /// Spawns the discovery service.
    ///
    /// This will spawn the [`reth_discv4::Discv4Service`] onto a new task and establish a listener
    /// channel to receive all discovered nodes.
    pub async fn new(
        tcp_addr: SocketAddr,
        discovery_v4_addr: SocketAddr,
        sk: SecretKey,
        discv4_config: Option<Discv4Config>,
        mut discv5_config: Option<reth_discv5::Config>, // contains discv5 listen address
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        // setup discv4 with the discovery address and tcp port
        let local_enr =
            NodeRecord::from_secret_key(discovery_v4_addr, &sk).with_tcp_port(tcp_addr.port());

        // For IPv6 we set IPV6_V6ONLY=true so an IPv4 sibling socket on the same port doesn't
        // clash with the IPv6 one (Linux's default of V6ONLY=0 has IPv6 also claim the IPv4
        // port via mapped addresses), matching how discv5 binds its `DualStack` sockets.
        let bind_socket = async |addr: SocketAddr| {
            let result = match addr {
                SocketAddr::V4(_) => UdpSocket::bind(addr).await,
                SocketAddr::V6(_) => {
                    use socket2::{Domain, Protocol, Socket, Type};
                    (|| {
                        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
                        socket.set_only_v6(true)?;
                        socket.set_nonblocking(true)?;
                        socket.bind(&addr.into())?;
                        UdpSocket::from_std(socket.into())
                    })()
                }
            };
            result
                .map(Arc::new)
                .map_err(|err| NetworkError::from_io_error(err, ServiceKind::Discovery(addr)))
        };

        // In shared-port mode, bind the shared socket and start discv4 without its own receive
        // loop. Unrecognized frames from discv5 will be forwarded to the ingress handler.
        let (discv4, discv4_updates, _discv4_service, discv4_ingress, shared_socket) =
            if let Some(config) = discv4_config {
                if let Some(discv5_config) = &mut discv5_config &&
                    discv5_config.has_matching_socket(discovery_v4_addr)
                {
                    let socket = bind_socket(discovery_v4_addr).await?;

                    let (discv4, mut discv4_service, ingress) = Discv4::bind_shared(
                        socket.clone(),
                        local_enr,
                        sk,
                        config,
                    )
                    .map_err(|err| {
                        NetworkError::from_io_error(err, ServiceKind::Discovery(discovery_v4_addr))
                    })?;

                    let discv4_updates = discv4_service.update_stream();
                    let discv4_service = discv4_service.spawn();
                    debug!(target:"net", ?discovery_v4_addr, "started discovery v4 (shared port)");
                    (
                        Some(discv4),
                        Some(discv4_updates),
                        Some(discv4_service),
                        Some(ingress),
                        Some(socket),
                    )
                } else {
                    let (discv4, mut discv4_service) =
                        Discv4::bind(discovery_v4_addr, local_enr, sk, config).await.map_err(
                            |err| {
                                NetworkError::from_io_error(
                                    err,
                                    ServiceKind::Discovery(discovery_v4_addr),
                                )
                            },
                        )?;
                    let discv4_updates = discv4_service.update_stream();
                    // spawn the service
                    let discv4_service = discv4_service.spawn();

                    debug!(target:"net", ?discovery_v4_addr, "started discovery v4");

                    (Some(discv4), Some(discv4_updates), Some(discv4_service), None, None)
                }
            } else {
                (None, None, None, None, None)
            };

        // Start discv5, wiring in the shared socket if in shared-port mode.
        let (discv5, discv5_updates) = if let Some(mut config) = discv5_config {
            if let Some(socket) = shared_socket {
                let discv5_cfg = config.discv5_config_mut();

                // The shared socket covers discv4's address family; bind the opposite family
                // only if discv5 was configured for dual-stack.
                let (mut ipv4, mut ipv6) = (None, None);
                if discovery_v4_addr.is_ipv4() {
                    ipv4 = Some(socket);
                    if let Some(addr) = reth_discv5::config::ipv6(&discv5_cfg.listen_config) {
                        ipv6 = Some(bind_socket(SocketAddr::V6(addr)).await?);
                    }
                } else {
                    ipv6 = Some(socket);
                    if let Some(addr) = reth_discv5::config::ipv4(&discv5_cfg.listen_config) {
                        ipv4 = Some(bind_socket(SocketAddr::V4(addr)).await?);
                    }
                }

                discv5_cfg.listen_config = discv5::ListenConfig::FromSockets { ipv4, ipv6 };
            }

            let (discv5, discv5_updates) = Discv5::start(&sk, config).await?;
            debug!(target:"net", discovery_v5_enr=?discv5.local_enr(), "started discovery v5");
            (Some(discv5), Some(discv5_updates))
        } else {
            (None, None)
        };

        // In shared-port mode, spawn a task that peels `UnrecognizedFrame` events off the discv5
        // update stream and feeds them into discv4's ingress. Other events are forwarded through
        // a new channel that `Discovery::poll` reads. This keeps both protocols moving without
        // requiring the main `Discovery::poll` loop to be driven for packets to be routed.
        let (discv5_updates, _discv5_forwarder) = match (discv4_ingress, discv5_updates) {
            (Some(mut ingress), Some(mut updates)) => {
                let (tx, rx) = mpsc::channel(updates.max_capacity());
                let handle = tokio::spawn(async move {
                    while let Some(event) = updates.recv().await {
                        if let discv5::Event::UnrecognizedFrame(frame) = &event {
                            ingress.handle_packet(&frame.packet, frame.src_address).await;
                            continue;
                        }
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                });
                (Some(ReceiverStream::new(rx)), Some(handle))
            }
            (_, updates) => (updates.map(ReceiverStream::new), None),
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
            discovery_listeners: Default::default(),
            local_enr,
            discv4,
            discv4_updates,
            _discv4_service,
            discv5,
            discv5_updates,
            _discv5_forwarder,
            discovered_nodes: LruMap::new(DEFAULT_MAX_CAPACITY_DISCOVERED_PEERS_CACHE),
            queued_events: Default::default(),
            _dns_disc_service,
            _dns_discovery,
            dns_discovery_updates,
        })
    }

    /// Registers a listener for receiving [`DiscoveryEvent`] updates.
    pub(crate) fn add_listener(&mut self, tx: mpsc::UnboundedSender<DiscoveryEvent>) {
        self.discovery_listeners.push(tx);
    }

    /// Notifies all registered listeners with the provided `event`.
    #[inline]
    fn notify_listeners(&mut self, event: &DiscoveryEvent) {
        self.discovery_listeners.retain_mut(|listener| listener.send(event.clone()).is_ok());
    }

    /// Updates the `eth:ForkId` field in discv4/discv5.
    pub(crate) fn update_fork_id(&self, fork_id: ForkId) {
        if let Some(discv4) = &self.discv4 {
            // use forward-compatible forkid entry
            discv4.set_eip868_rlp(b"eth".to_vec(), EnrForkIdEntry::from(fork_id))
        }
        if let Some(discv5) = &self.discv5 {
            discv5
                .encode_and_set_eip868_in_local_enr(b"eth".to_vec(), EnrForkIdEntry::from(fork_id))
        }
    }

    /// Bans the [`IpAddr`] in the discovery service.
    pub(crate) fn ban_ip(&self, ip: IpAddr) {
        if let Some(discv4) = &self.discv4 {
            discv4.ban_ip(ip)
        }
        if let Some(discv5) = &self.discv5 {
            discv5.ban_ip(ip)
        }
    }

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    pub(crate) fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        if let Some(discv4) = &self.discv4 {
            discv4.ban(peer_id, ip)
        }
        if let Some(discv5) = &self.discv5 {
            discv5.ban(peer_id, ip)
        }
    }

    /// Returns a shared reference to the discv4.
    pub fn discv4(&self) -> Option<Discv4> {
        self.discv4.clone()
    }

    /// Returns the id with which the local node identifies itself in the network
    pub(crate) const fn local_id(&self) -> PeerId {
        self.local_enr.id // local discv4 and discv5 have same id, since signed with same secret key
    }

    /// Add a node to the discv4 table.
    pub(crate) fn add_discv4_node(&self, node: NodeRecord) {
        if let Some(discv4) = &self.discv4 {
            discv4.add_node(node);
        }
    }

    /// Returns discv5 handle.
    pub fn discv5(&self) -> Option<Discv5> {
        self.discv5.clone()
    }

    /// Add a node to the discv4 table.
    pub(crate) fn add_discv5_node(&self, enr: Enr<SecretKey>) -> Result<(), NetworkError> {
        if let Some(discv5) = &self.discv5 {
            discv5.add_node(enr).map_err(NetworkError::Discv5Error)?;
        }

        Ok(())
    }

    /// Processes an incoming [`NodeRecord`] update from a discovery service
    fn on_node_record_update(&mut self, record: NodeRecord, fork_id: Option<ForkId>) {
        let peer_id = record.id;
        let tcp_addr = record.tcp_addr();
        if tcp_addr.port() == 0 {
            // useless peer for p2p
            return
        }
        let udp_addr = record.udp_addr();
        let addr = PeerAddr::new(tcp_addr, Some(udp_addr));
        _ =
            self.discovered_nodes.get_or_insert(peer_id, || {
                self.queued_events.push_back(DiscoveryEvent::NewNode(
                    DiscoveredEvent::EventQueued { peer_id, addr, fork_id },
                ));

                addr
            })
    }

    fn on_discv4_update(&mut self, update: DiscoveryUpdate) {
        match update {
            DiscoveryUpdate::Added(record) | DiscoveryUpdate::DiscoveredAtCapacity(record) => {
                self.on_node_record_update(record, None);
            }
            DiscoveryUpdate::EnrForkId(node, fork_id) => {
                self.queued_events.push_back(DiscoveryEvent::EnrForkId(node, fork_id))
            }
            DiscoveryUpdate::Removed(peer_id) => {
                self.discovered_nodes.remove(&peer_id);
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
                self.notify_listeners(&event);
                return Poll::Ready(event)
            }

            // drain the discv4 update stream
            while let Some(Poll::Ready(Some(update))) =
                self.discv4_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
            {
                self.on_discv4_update(update)
            }

            // drain the discv5 update stream
            while let Some(Poll::Ready(Some(update))) =
                self.discv5_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
            {
                if let Some(discv5) = self.discv5.as_mut() &&
                    let Some(DiscoveredPeer { node_record, fork_id }) =
                        discv5.on_discv5_update(update)
                {
                    self.on_node_record_update(node_record, fork_id);
                }
            }

            // drain the dns update stream
            while let Some(Poll::Ready(Some(update))) =
                self.dns_discovery_updates.as_mut().map(|updates| updates.poll_next_unpin(cx))
            {
                self.add_discv4_node(update.node_record);
                if let Err(err) = self.add_discv5_node(update.enr) {
                    trace!(target: "net::discovery",
                        %err,
                        "failed adding node discovered by dns to discv5"
                    );
                }
                self.on_node_record_update(update.node_record, update.fork_id);
            }

            if self.queued_events.is_empty() {
                return Poll::Pending
            }
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(discv4) = &self.discv4 {
            discv4.terminate();
        }
        if let Some(handle) = self._discv4_service.take() {
            handle.abort();
        }
        if let Some(handle) = self._discv5_forwarder.take() {
            handle.abort();
        }
        if let Some(handle) = self._dns_disc_service.take() {
            handle.abort();
        }
    }
}

impl Stream for Discovery {
    type Item = DiscoveryEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(ready!(self.get_mut().poll(cx))))
    }
}

#[cfg(test)]
impl Discovery {
    /// Returns a Discovery instance that does nothing and is intended for testing purposes.
    ///
    /// NOTE: This instance does nothing
    pub(crate) fn noop() -> Self {
        let (_discovery_listeners, _): (mpsc::UnboundedSender<DiscoveryEvent>, _) =
            mpsc::unbounded_channel();

        Self {
            discovered_nodes: LruMap::new(0),
            local_enr: NodeRecord {
                address: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
                tcp_port: 0,
                udp_port: 0,
                id: PeerId::random(),
            },
            discv4: Default::default(),
            discv4_updates: Default::default(),
            _discv4_service: Default::default(),
            _discv5_forwarder: None,
            discv5: None,
            discv5_updates: None,
            queued_events: Default::default(),
            _dns_discovery: None,
            dns_discovery_updates: None,
            _dns_disc_service: None,
            discovery_listeners: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secp256k1::SECP256K1;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[tokio::test(flavor = "multi_thread")]
    async fn test_discovery_setup() {
        let (secret_key, _) = SECP256K1.generate_keypair(&mut rand_08::thread_rng());
        let discovery_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        let _discovery = Discovery::new(
            discovery_addr,
            discovery_addr,
            secret_key,
            Default::default(),
            None,
            Default::default(),
        )
        .await
        .unwrap();
    }

    use reth_discv4::Discv4ConfigBuilder;
    use reth_discv5::{enr::EnrCombinedKeyWrapper, enr_to_discv4_id};
    use tracing::trace;

    async fn start_discovery_node(udp_port_discv4: u16, udp_port_discv5: u16) -> Discovery {
        let secret_key = SecretKey::new(&mut rand_08::thread_rng());

        let discv4_addr = format!("127.0.0.1:{udp_port_discv4}").parse().unwrap();
        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        // disable `NatResolver`
        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = reth_discv5::Config::builder(discv5_addr)
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        Discovery::new(
            discv4_addr,
            discv4_addr,
            secret_key,
            Some(discv4_config),
            Some(discv5_config),
            None,
        )
        .await
        .expect("should build discv5 with discv4 downgrade")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discv5_and_discv4_same_pk() {
        reth_tracing::init_test_tracing();

        // set up test
        let mut node_1 = start_discovery_node(40014, 40015).await;
        let discv4_enr_1 = node_1.discv4.as_ref().unwrap().node_record();
        let discv5_enr_node_1 =
            node_1.discv5.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let discv4_id_1 = discv4_enr_1.id;
        let discv5_id_1 = discv5_enr_node_1.node_id();

        let mut node_2 = start_discovery_node(40024, 40025).await;
        let discv4_enr_2 = node_2.discv4.as_ref().unwrap().node_record();
        let discv5_enr_node_2 =
            node_2.discv5.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let discv4_id_2 = discv4_enr_2.id;
        let discv5_id_2 = discv5_enr_node_2.node_id();

        trace!(target: "net::discovery::tests",
            node_1_node_id=format!("{:#}", discv5_id_1),
            node_2_node_id=format!("{:#}", discv5_id_2),
            "started nodes"
        );

        // test

        // assert discovery version 4 and version 5 nodes have same id
        assert_eq!(discv4_id_1, enr_to_discv4_id(&discv5_enr_node_1).unwrap());
        assert_eq!(discv4_id_2, enr_to_discv4_id(&discv5_enr_node_2).unwrap());

        // add node_2:discv4 manually to node_1:discv4
        node_1.add_discv4_node(discv4_enr_2);

        // verify node_2:discv4 discovered node_1:discv4 and vv
        let event_node_1 = node_1.next().await.unwrap();
        let event_node_2 = node_2.next().await.unwrap();

        assert_eq!(
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued {
                peer_id: discv4_id_2,
                addr: PeerAddr::new(discv4_enr_2.tcp_addr(), Some(discv4_enr_2.udp_addr())),
                fork_id: None
            }),
            event_node_1
        );
        assert_eq!(
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued {
                peer_id: discv4_id_1,
                addr: PeerAddr::new(discv4_enr_1.tcp_addr(), Some(discv4_enr_1.udp_addr())),
                fork_id: None
            }),
            event_node_2
        );

        assert_eq!(1, node_1.discovered_nodes.len());
        assert_eq!(1, node_2.discovered_nodes.len());

        // add node_2:discv5 to node_1:discv5, manual insertion won't emit an event
        node_1.add_discv5_node(EnrCombinedKeyWrapper(discv5_enr_node_2.clone()).into()).unwrap();
        // verify node_2 is in KBuckets of node_1:discv5
        assert!(node_1
            .discv5
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.table_entries_id().contains(&discv5_id_2)));

        // manually trigger connection from node_1:discv5 to node_2:discv5
        node_1
            .discv5
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.send_ping(discv5_enr_node_2.clone()))
            .await
            .unwrap();

        // this won't emit an event, since the nodes already discovered each other on discv4, the
        // number of nodes stored for each node on this level remains 1.
        assert_eq!(1, node_1.discovered_nodes.len());
        assert_eq!(1, node_2.discovered_nodes.len());
    }

    /// Starts a discovery node with discv4 and discv5 sharing the same UDP port.
    async fn start_shared_port_node(port: u16) -> Discovery {
        let secret_key = SecretKey::new(&mut rand_08::thread_rng());
        let disc_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        // Use a non-zero TCP port so the node record isn't filtered out by
        // `on_node_record_update` (which drops peers with tcp port == 0).
        let tcp_addr: SocketAddr = "127.0.0.1:30303".parse().unwrap();

        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(disc_addr);
        let discv5_config = reth_discv5::Config::builder(tcp_addr)
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        // Both protocols use the same address, triggering shared-port mode
        Discovery::new(
            tcp_addr,
            disc_addr,
            secret_key,
            Some(discv4_config),
            Some(discv5_config),
            None,
        )
        .await
        .expect("should start with shared port")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_shared_port_setup() {
        reth_tracing::init_test_tracing();

        // Use port 0 so the OS picks a free port
        let node = start_shared_port_node(0).await;

        // Both protocols should be active
        assert!(node.discv4.is_some(), "discv4 should be running");
        assert!(node.discv5.is_some(), "discv5 should be running");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_shared_port_discv5_discovery() {
        reth_tracing::init_test_tracing();

        let mut node_1 = start_shared_port_node(0).await;
        let mut node_2 = start_shared_port_node(0).await;

        let discv5_enr_1 = node_1.discv5.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let discv5_enr_2 = node_2.discv5.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());

        let peer_id_1 = enr_to_discv4_id(&discv5_enr_1).unwrap();
        let peer_id_2 = enr_to_discv4_id(&discv5_enr_2).unwrap();

        // Add node_2's ENR to node_1's discv5 kbuckets and trigger a ping to establish a session.
        // send_ping awaits the PONG, so the handshake completes before we poll the Discovery
        // stream. The discv5 service runs its own background task.
        node_1.add_discv5_node(EnrCombinedKeyWrapper(discv5_enr_2.clone()).into()).unwrap();
        node_1
            .discv5
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.send_ping(discv5_enr_2))
            .await
            .unwrap();

        // Both SessionEstablished events should now be buffered in the update channels.
        // Drive both nodes concurrently to collect them.
        let mut event_1 = None;
        let mut event_2 = None;
        let timeout = tokio::time::sleep(std::time::Duration::from_secs(5));
        tokio::pin!(timeout);
        loop {
            tokio::select! {
                ev = node_1.next(), if event_1.is_none() => {
                    event_1 = ev;
                }
                ev = node_2.next(), if event_2.is_none() => {
                    event_2 = ev;
                }
                _ = &mut timeout => {
                    panic!("timed out waiting for discv5 discovery events");
                }
            }
            if event_1.is_some() && event_2.is_some() {
                break;
            }
        }

        assert!(matches!(
            event_1.unwrap(),
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued { peer_id, .. })
                if peer_id == peer_id_2
        ));
        assert!(matches!(
            event_2.unwrap(),
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued { peer_id, .. })
                if peer_id == peer_id_1
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_shared_port_discv4_discovery() {
        reth_tracing::init_test_tracing();

        let mut node_1 = start_shared_port_node(0).await;
        let mut node_2 = start_shared_port_node(0).await;

        let enr_1 = node_1.discv4.as_ref().unwrap().node_record();
        let enr_2 = node_2.discv4.as_ref().unwrap().node_record();

        // Introduce node_2 to node_1 via discv4
        node_1.add_discv4_node(enr_2);

        // Both nodes should discover each other via discv4 ping/pong
        let event_1 = node_1.next().await.unwrap();
        let event_2 = node_2.next().await.unwrap();

        assert_eq!(
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued {
                peer_id: enr_2.id,
                addr: PeerAddr::new(enr_2.tcp_addr(), Some(enr_2.udp_addr())),
                fork_id: None
            }),
            event_1
        );
        assert_eq!(
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued {
                peer_id: enr_1.id,
                addr: PeerAddr::new(enr_1.tcp_addr(), Some(enr_1.udp_addr())),
                fork_id: None
            }),
            event_2
        );
    }

    /// Verifies that shared-port mode binds correctly when discv5 is configured for dual-stack.
    /// On Linux this exercises the IPv6 V6ONLY path: without it, the IPv4 sibling would clash
    /// with the IPv6 socket bound to the same port.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_shared_port_dual_stack() {
        reth_tracing::init_test_tracing();

        // Find a port that's free on the v4 wildcard so we can use it for both v4 and v6.
        let probe = UdpSocket::bind("0.0.0.0:0").await.expect("probe bind");
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let secret_key = SecretKey::new(&mut rand_08::thread_rng());
        let v4_addr: SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
        let tcp_addr: SocketAddr = "0.0.0.0:30303".parse().unwrap();

        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::DualStack {
            ipv4: std::net::Ipv4Addr::UNSPECIFIED,
            ipv4_port: port,
            ipv6: std::net::Ipv6Addr::UNSPECIFIED,
            ipv6_port: port,
        };
        let discv5_config = reth_discv5::Config::builder(tcp_addr)
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        Discovery::new(
            tcp_addr,
            v4_addr,
            secret_key,
            Some(discv4_config),
            Some(discv5_config),
            None,
        )
        .await
        .expect("discovery should start with shared port + dual-stack");
    }
}
