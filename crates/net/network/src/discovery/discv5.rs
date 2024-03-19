//! Discovery support for the network using [`discv5::Discv5`].

use crate::error::NetworkError;
use discv5::{enr::Enr, IpMode};
use futures::StreamExt;
use reth_discv4::SecretKey;
use reth_discv5::{
    enr::{uncompressed_id_from_enr_pk, EnrCombinedKeyWrapper},
    DiscV5, DiscV5Config, HandleDiscv5,
};
use reth_dns_discovery::{new_with_dns_resolver, DnsDiscoveryConfig};
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::NodeRecord;
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::{error, trace};

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use super::{Discovery, DiscoveryEvent};

/// [`Discovery`] type that uses [`discv5::Discv5`].
#[cfg(feature = "discv5")]
pub type DiscoveryV5 = Discovery<DiscV5, ReceiverStream<discv5::Event>, Enr<SecretKey>>;

impl Discovery<DiscV5, ReceiverStream<discv5::Event>, Enr<SecretKey>> {
    /// Spawns the discovery service.
    ///
    /// This will spawn [`discv5::Discv5`] and establish a listener channel to receive all /
    /// discovered nodes.
    pub async fn start_discv5(
        sk: SecretKey,
        discv5_config: Option<DiscV5Config>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        let (disc, disc_updates, bc_local_enr) = match discv5_config {
            Some(config) => {
                let (disc, disc_updates, bc_local_enr) = start_discv5(&sk, config).await?;

                (Some(disc), Some(disc_updates.into()), bc_local_enr)
            }
            None => (None, None, NodeRecord::from_secret_key("0.0.0.0:0".parse().unwrap(), &sk)),
        };

        // setup DNS discovery.
        let (_dns_discovery, dns_discovery_updates, _dns_disc_service) =
            if let Some(dns_config) = dns_discovery_config {
                new_with_dns_resolver::<Enr<SecretKey>>(dns_config)?
            } else {
                (None, None, None)
            };

        Ok(Discovery {
            discovery_listeners: Default::default(),
            local_enr: bc_local_enr,
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

#[cfg(feature = "discv5")]
impl Discovery<DiscV5, ReceiverStream<discv5::Event>, Enr<SecretKey>> {
    pub async fn start(
        _discv4_addr: std::net::SocketAddr,
        sk: SecretKey,
        _discv4_config: Option<reth_discv4::Discv4Config>,
        discv5_config: Option<DiscV5Config>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        Discovery::start_discv5(sk, discv5_config, dns_discovery_config).await
    }

    /// Returns a shared reference to the [`DiscV5`] handle.
    pub fn discv5(&self) -> Option<DiscV5> {
        self.disc.clone()
    }
}

impl<D, S, N> Discovery<D, S, N>
where
    D: HandleDiscovery + HandleDiscv5,
{
    pub fn on_discv5_update(&mut self, update: discv5::Event) -> Result<(), NetworkError> {
        match update {
            discv5::Event::Discovered(enr) => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::DiscoveredAtCapacity(_)

                // node has been discovered as part of a query. discv5::Config sets
                // `report_discovered_peers` to true by default.

                self.try_insert_enr_into_discovered_nodes(enr)?;
            }
            discv5::Event::EnrAdded { .. } => {
                // not used in discv5 codebase
            }
            discv5::Event::NodeInserted { replaced, .. } => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::Removed(_)

                if let Some(ref disc) = self.disc {
                    if let Some(ref node_id) = replaced {
                        if let Some(peer_id) = disc.with_discv5(|discv5| {
                            discv5.with_kbuckets(|kbuckets| {
                                kbuckets
                                    .read()
                                    .iter_ref()
                                    .find(|entry| entry.node.key.preimage() == node_id)
                                    .map(|entry| uncompressed_id_from_enr_pk(entry.node.value))
                            })
                        }) {
                            self.discovered_nodes.remove(&peer_id);
                        }
                    }
                }
            }
            discv5::Event::SessionEstablished(enr, _remote_socket) => {
                // covers DiscoveryUpdate::Added(_) and DiscoveryUpdate::DiscoveredAtCapacity(_)

                // node has been discovered unrelated to a query, e.g. an incoming connection to
                // discv5

                self.try_insert_enr_into_discovered_nodes(enr)?;
            }
            discv5::Event::SocketUpdated(_socket_addr) => {}
            discv5::Event::TalkRequest(_talk_req) => {}
        }

        Ok(())
    }

    fn try_insert_enr_into_discovered_nodes(
        &mut self,
        enr: discv5::Enr,
    ) -> Result<(), NetworkError> {
        let Some(ref discv5) = self.disc else { return Ok(()) };
        let Some(udp_socket) = discv5.ip_mode().get_contactable_addr(&enr) else {
            trace!(target: "net::discovery",
                %enr,
                "received ENR that is not WAN-reachable"
            );

            return Ok(())
        };
        // todo: get tcp port v6 with respect to ip mode of local node
        let tcp_port = enr.tcp4().unwrap_or_else(|| enr.tcp6().unwrap_or(0));

        let id = uncompressed_id_from_enr_pk(&enr);

        let record =
            NodeRecord { address: udp_socket.ip(), tcp_port, udp_port: udp_socket.port(), id };

        self.on_node_record_update(record, None);

        Ok(())
    }
}

impl<S> Stream for Discovery<DiscV5, S, Enr<SecretKey>>
where
    S: Stream<Item = discv5::Event> + Unpin + Send + 'static,
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
            if let Err(err) = self.on_discv5_update(update) {
                error!(target: "net::discovery", %err, "failed to process update");
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

        if self.queued_events.is_empty() {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}

/// Spawns [`discv5::Discv5`].
pub(super) async fn start_discv5(
    sk: &SecretKey,
    discv5_config: DiscV5Config,
) -> Result<(DiscV5, mpsc::Receiver<discv5::Event>, NodeRecord), NetworkError> {
    //
    // 1. make local enr from listen config
    //
    let (discv5_config, bootstrap_nodes, fork_id) = discv5_config.destruct();

    let (enr, bc_enr, ip_mode) = {
        let mut builder = discv5::enr::Enr::builder();

        use discv5::ListenConfig::*;
        let ip_mode = match discv5_config.listen_config {
            Ipv4 { ip, port } => {
                builder.ip4(ip);
                builder.udp4(port);

                IpMode::Ip4
            }
            Ipv6 { ip, port } => {
                builder.ip6(ip);
                builder.udp6(port);

                IpMode::Ip6
            }
            DualStack { ipv4, ipv4_port, ipv6, ipv6_port } => {
                builder.ip4(ipv4);
                builder.udp4(ipv4_port);
                builder.ip6(ipv6);
                builder.udp6(ipv6_port);

                IpMode::DualStack
            }
        };

        builder.add_value("eth", &alloy_rlp::encode(fork_id));

        // enr v4 not to get confused with discv4, independent versioning enr and
        // discovery
        let enr = builder.build(sk).expect("should build enr v4");
        let EnrCombinedKeyWrapper(enr) = enr.into();

        // backwards compatible enr
        let socket = ip_mode.get_contactable_addr(&enr).unwrap();
        let bc_enr = NodeRecord::from_secret_key(socket, sk);

        (enr, bc_enr, ip_mode)
    };

    //
    // 3. start discv5
    //
    let sk = {
        let mut sk = *sk.as_ref();
        discv5::enr::CombinedKey::secp256k1_from_bytes(&mut sk)
            .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?
    };

    let mut discv5 =
        discv5::Discv5::new(enr, sk, discv5_config).map_err(NetworkError::custom_discovery)?;
    discv5.start().await.map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;

    // start discv5 updates stream
    let discv5_updates =
        discv5.event_stream().await.map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;

    //
    // 4. add boot nodes
    //
    for node in bootstrap_nodes {
        discv5.add_enr(node).map_err(NetworkError::custom_discovery)?;
    }

    Ok((DiscV5::new(Arc::new(discv5), ip_mode), discv5_updates, bc_enr))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use rand::thread_rng;
    use reth_discv5::DiscV5ConfigBuilder;
    use tracing::trace;

    use super::*;

    async fn start_discovery_node(
        udp_port_discv5: u16,
    ) -> Discovery<DiscV5, ReceiverStream<discv5::Event>, enr::Enr<secp256k1::SecretKey>> {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = DiscV5ConfigBuilder::default()
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        Discovery::start_discv5(secret_key, Some(discv5_config), None)
            .await
            .expect("should build discv5")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discv5() {
        reth_tracing::init_test_tracing();

        let mut node_1 = start_discovery_node(30344).await;
        let node_1_enr = node_1.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());

        let mut node_2 = start_discovery_node(30355).await;
        let node_2_enr = node_2.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());

        trace!(target: "net::discovery::discv5_tests",
            node_1_node_id=format!("{:#}", node_1_enr.node_id()),
            node_2_node_id=format!("{:#}", node_2_enr.node_id()),
            "started nodes"
        );

        // add node_2 to discovery handle of node_1 (should add node to discv5 kbuckets)
        let node_2_enr_reth_compatible_ty: Enr<SecretKey> =
            EnrCombinedKeyWrapper(node_2_enr.clone()).into();
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
        matches!(event_1_v5, discv5::Event::SessionEstablished(node, socket) if node == node_2_enr && socket == node_2_enr.udp4_socket().unwrap().into());
        matches!(event_2_v5, discv5::Event::SessionEstablished(node, socket) if node == node_1_enr && socket == node_1_enr.udp4_socket().unwrap().into());

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_2_v5, discv5::Event::NodeInserted { node_id, replaced } if node_id == node_1_enr.node_id() && replaced.is_none());
    }
}
