//! Discovery support for the network.

use crate::{
    cache::LruMap,
    error::{NetworkError, ServiceKind},
    manager::DiscoveredEvent,
};
use enr::Enr;
use futures::StreamExt;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4Config};
use reth_discv5::{DiscoveredPeer, Discv5};
use reth_dns_discovery::{
    DnsDiscoveryConfig, DnsDiscoveryHandle, DnsDiscoveryService, DnsNodeRecordUpdate, DnsResolver,
};
use reth_network_types::PeerId;
use reth_primitives::{EnrForkIdEntry, ForkId, NodeRecord};
use secp256k1::SecretKey;
use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::trace;

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
    discovered_nodes: LruMap<PeerId, SocketAddr>,
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
        discovery_v4_addr: SocketAddr,
        sk: SecretKey,
        discv4_config: Option<Discv4Config>,
        discv5_config: Option<reth_discv5::Config>, // contains discv5 listen address
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        // setup discv4
        let local_enr = NodeRecord::from_secret_key(discovery_v4_addr, &sk);
        let discv4_future = async {
            let Some(disc_config) = discv4_config else { return Ok((None, None, None)) };
            let (discv4, mut discv4_service) =
                Discv4::bind(discovery_v4_addr, local_enr, sk, disc_config).await.map_err(
                    |err| {
                        NetworkError::from_io_error(err, ServiceKind::Discovery(discovery_v4_addr))
                    },
                )?;
            let discv4_updates = discv4_service.update_stream();
            // spawn the service
            let discv4_service = discv4_service.spawn();

            Ok((Some(discv4), Some(discv4_updates), Some(discv4_service)))
        };

        let discv5_future = async {
            let Some(config) = discv5_config else { return Ok::<_, NetworkError>((None, None)) };
            let (discv5, discv5_updates, _local_enr_discv5) = Discv5::start(&sk, config).await?;
            Ok((Some(discv5), Some(discv5_updates.into())))
        };

        let ((discv4, discv4_updates, _discv4_service), (discv5, discv5_updates)) =
            tokio::try_join!(discv4_future, discv5_future)?;

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
            discovered_nodes: LruMap::new(DEFAULT_MAX_CAPACITY_DISCOVERED_PEERS_CACHE),
            queued_events: Default::default(),
            _dns_disc_service,
            _dns_discovery,
            dns_discovery_updates,
        })
    }

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
        if let Some(discv4) = &self.discv4 {
            // use forward-compatible forkid entry
            discv4.set_eip868_rlp("eth".as_bytes().to_vec(), EnrForkIdEntry::from(fork_id))
        }
        // todo: update discv5 enr
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
    pub(crate) fn local_id(&self) -> PeerId {
        self.local_enr.id // local discv4 and discv5 have same id, since signed with same secret key
    }

    /// Add a node to the discv4 table.
    pub(crate) fn add_discv4_node(&self, node: NodeRecord) {
        if let Some(discv4) = &self.discv4 {
            discv4.add_node(node);
        }
    }

    /// Add a node to the discv4 table.
    pub(crate) fn add_discv5_node(&self, enr: Enr<SecretKey>) -> Result<(), NetworkError> {
        if let Some(discv5) = &self.discv5 {
            discv5.add_node(enr).map_err(NetworkError::Discv5Error)?;
        }

        Ok(())
    }

    /// Processes an incoming [NodeRecord] update from a discovery service
    fn on_node_record_update(&mut self, record: NodeRecord, fork_id: Option<ForkId>) {
        let id = record.id;
        let addr = record.tcp_addr();
        _ =
            self.discovered_nodes.get_or_insert(id, || {
                self.queued_events.push_back(DiscoveryEvent::NewNode(
                    DiscoveredEvent::EventQueued { peer_id: id, socket_addr: addr, fork_id },
                ));

                addr
            })
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
                if let Some(discv5) = self.discv5.as_mut() {
                    if let Some(DiscoveredPeer { node_record, fork_id }) =
                        discv5.on_discv5_update(update)
                    {
                        self.on_node_record_update(node_record, fork_id);
                    }
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
            discv5: None,
            discv5_updates: None,
            queued_events: Default::default(),
            _discv4_service: Default::default(),
            _dns_discovery: None,
            dns_discovery_updates: None,
            _dns_disc_service: None,
            discovery_listeners: Default::default(),
        }
    }
}

/// Events produced by the [`Discovery`] manager.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        let _discovery = Discovery::new(
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
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv4_addr = format!("127.0.0.1:{udp_port_discv4}").parse().unwrap();
        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        // disable `NatResolver`
        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = reth_discv5::Config::builder(discv5_addr)
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        Discovery::new(discv4_addr, secret_key, Some(discv4_config), Some(discv5_config), None)
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
                socket_addr: discv4_enr_2.tcp_addr(),
                fork_id: None
            }),
            event_node_1
        );
        assert_eq!(
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued {
                peer_id: discv4_id_1,
                socket_addr: discv4_enr_1.tcp_addr(),
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
}
