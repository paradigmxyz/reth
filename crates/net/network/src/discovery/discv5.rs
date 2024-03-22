//! Discovery support for the network using [`discv5::Discv5`].

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::StreamExt;
use reth_discv4::secp256k1::SecretKey;
use reth_discv5::{
    discv5::{self, enr::Enr},
    enr::uncompressed_id_from_enr_pk,
    filter::FilterOutcome,
    DiscV5, DiscV5Config, HandleDiscv5,
};
use reth_dns_discovery::{new_with_dns_resolver, DnsDiscoveryConfig};
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::NodeRecord;

use tokio_stream::{wrappers::ReceiverStream, Stream};
use tracing::{error, trace};

use crate::error::NetworkError;

use super::{Discovery, DiscoveryEvent};

/// [`Discovery`] type that uses [`discv5::Discv5`].
#[cfg(feature = "discv5")]
pub type DiscoveryV5 = Discovery<DiscV5<T>, ReceiverStream<discv5::Event>, Enr<SecretKey>>;

impl Discovery<DiscV5<T>, ReceiverStream<discv5::Event>, Enr<SecretKey>> {
    /// Spawns the discovery service.
    ///
    /// This will spawn [`discv5::Discv5`] and establish a listener channel to receive all /
    /// discovered nodes.
    pub async fn start_discv5(
        sk: SecretKey,
        discv5_config: Option<DiscV5Config>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        trace!(target: "net::discovery::discv5",
            "starting discovery .."
        );

        let (disc, disc_updates, bc_local_enr) = match discv5_config {
            Some(config) => {
                let (disc, disc_updates, bc_local_enr) = DiscV5::start(&sk, config)
                    .await
                    .map_err(|e| NetworkError::custom_discovery(&e.to_string()))?;

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
impl<T> Discovery<DiscV5<T>, ReceiverStream<discv5::Event>, Enr<SecretKey>> {
    pub async fn start(
        _discv4_addr: std::net::SocketAddr,
        sk: SecretKey,
        _discv4_config: Option<reth_discv4::Discv4Config>,
        discv5_config: Option<DiscV5Config<T>>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError> {
        let discv5_config = Some(DiscV5ConfigBuilder::new_from(discv5_config).filter(MustIncludeChain::new(b"opstack")).add_enode_boot_nodes("enode://ca2774c3c401325850b2477fd7d0f27911efbf79b1e8b335066516e2bd8c4c9e0ba9696a94b1cb030a88eac582305ff55e905e64fb77fe0edcd70a4e5296d3ec@34.65.175.185:30305,enode://dd751a9ef8912be1bfa7a5e34e2c3785cc5253110bd929f385e07ba7ac19929fb0e0c5d93f77827291f4da02b2232240fbc47ea7ce04c46e333e452f8656b667@34.65.107.0:30305,enode://c5d289b56a77b6a2342ca29956dfd07aadf45364dde8ab20d1dc4efd4d1bc6b4655d902501daea308f4d8950737a4e93a4dfedd17b49cd5760ffd127837ca965@34.65.202.239:30305,enode://87a32fd13bd596b2ffca97020e31aef4ddcc1bbd4b95bb633d16c1329f654f34049ed240a36b449fda5e5225d70fe40bc667f53c304b71f8e68fc9d448690b51@3.231.138.188:30301,enode://ca21ea8f176adb2e229ce2d700830c844af0ea941a1d8152a9513b966fe525e809c3a6c73a2c18a12b74ed6ec4380edf91662778fe0b79f6a591236e49e176f9@184.72.129.189:30301,enode://acf4507a211ba7c1e52cdf4eef62cdc3c32e7c9c47998954f7ba024026f9a6b2150cd3f0b734d9c78e507ab70d59ba61dfe5c45e1078c7ad0775fb251d7735a2@3.220.145.177:30301,enode://8a5a5006159bf079d06a04e5eceab2a1ce6e0f721875b2a9c96905336219dbe14203d38f70f3754686a6324f786c2f9852d8c0dd3adac2d080f4db35efc678c5@3.231.11.52:30301,enode://cdadbe835308ad3557f9a1de8db411da1a260a98f8421d62da90e71da66e55e98aaa8e90aa7ce01b408a54e4bd2253d701218081ded3dbe5efbbc7b41d7cef79@54.198.153.150:30301").fork_id(b"opstack", ForkId { hash: reth_primitives::ForkHash([0x51, 0xcc, 0x98, 0xb3]), next: 0 }).build());

        Discovery::start_discv5(sk, discv5_config, dns_discovery_config).await
    }

    /// Returns a shared reference to the [`DiscV5`] handle.
    pub fn discv5(&self) -> Option<DiscV5<T>> {
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

                self.on_discovered_peer(enr);
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

                self.on_discovered_peer(enr);
            }
            discv5::Event::SocketUpdated(_socket_addr) => {}
            discv5::Event::TalkRequest(_talk_req) => {}
        }

        Ok(())
    }

    fn on_discovered_peer(&mut self, enr: discv5::Enr) {
        let Some(ref discv5) = self.disc else { return };

        if let FilterOutcome::Ignore { reason } = discv5.filter_discovered_peer(&enr) {
            trace!(target: "net::discovery::discv5",
                ?enr,
                reason,
                "filtered out discovered peer"
            );
        }

        let fork_id = discv5.get_fork_id(&enr).ok();

        match discv5.try_into_reachable(enr.clone()) {
            Ok(enr_bc) => self.on_node_record_update(enr_bc, fork_id),
            Err(err) => {
                trace!(target: "net::discovery::discv5",
                        ?fork_id,
                        %err,
                        "discovered unreachable peer"
                );
                return
            }
        }
        trace!(target: "net::discovery::discv5",
                ?fork_id,
                ?enr,
                "discovered peer on opstack"
        );
    }
}

impl<S> Stream for Discovery<DiscV5<T>, S, Enr<SecretKey>>
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
                error!(target: "net::discovery::discv5", %err, "failed to process update");
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

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use rand::thread_rng;
    use reth_discv5::enr::EnrCombinedKeyWrapper;
    use tracing::trace;

    use super::*;

    async fn start_discovery_node(
        udp_port_discv5: u16,
    ) -> Discovery<DiscV5, ReceiverStream<discv5::Event>, enr::Enr<secp256k1::SecretKey>> {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = DiscV5Config::builder()
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

        trace!(target: "net::discovery::tests",
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
