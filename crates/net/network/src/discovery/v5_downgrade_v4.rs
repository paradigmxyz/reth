//! Discovery support for the network using [`discv5::Discv5`](reth_discv5::discv5), with support
//! for downgraded [`Discv4`](reth_discv4::Discv4) connections.

use std::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

use discv5::enr::Enr;
use futures::{Stream, StreamExt};
use reth_discv4::{secp256k1::SecretKey, Discv4Config};
use reth_discv5::{
    filter::{FilterDiscovered, MustNotIncludeChains},
    metrics::UpdateMetrics,
    Discv5BCv4, MergedUpdateStream,
};
use reth_dns_discovery::{new_with_dns_resolver, DnsDiscoveryConfig};
use reth_net_common::discovery::NodeFromExternalSource;
use tracing::trace;

use crate::error::NetworkError;

use super::{
    version::{CloneDiscoveryHandle, DiscoveryHandle},
    Discovery, DiscoveryEvent,
};

/// [`Discovery`] type that uses [`discv5::Discv5`](reth_discv5::discv5), with support for
/// downgraded [`Discv4`](reth_discv4::Discv4) connections.

pub type Discovery5BC<T = MustNotIncludeChains> =
    Discovery<Discv5BCv4<T>, MergedUpdateStream, Enr<SecretKey>>;

impl<T> Discovery<Discv5BCv4<T>, MergedUpdateStream, Enr<SecretKey>> {
    /// Spawns the discovery service.
    ///
    /// This will spawn [`discv5::Discv5`](reth_discv5::discv5) and [`Discv4`](reth_discv4::Discv4)
    /// each onto their own new task and establish a merged listener channel to receive all
    /// discovered nodes.
    pub async fn start_discv5_with_v4_downgrade(
        discv4_addr: SocketAddr, // discv5 addr in config
        sk: SecretKey,
        discv4_config: Discv4Config,
        discv5_config: reth_discv5::Config<T>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError>
    where
        T: FilterDiscovered + Clone + Send + Sync + 'static,
    {
        trace!(target: "net::discovery::discv5_downgrade_v4",
            "starting discovery .."
        );

        let (disc, disc_updates, bc_local_discv5_enr) =
            Discv5BCv4::start(discv4_addr, sk, discv4_config, discv5_config)
                .await
                .map_err(|err| NetworkError::custom_discovery(&err.to_string()))?;

        // setup DNS discovery.
        let (_dns_discovery, dns_discovery_updates, _dns_disc_service) =
            if let Some(dns_config) = dns_discovery_config {
                new_with_dns_resolver::<Enr<SecretKey>>(dns_config)?
            } else {
                (None, None, None)
            };

        Ok(Discovery {
            discovery_listeners: Default::default(),
            local_enr: bc_local_discv5_enr,
            disc: Some(disc),
            disc_updates: Some(disc_updates),
            _disc_service: None,
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
            _dns_disc_service,
            _dns_discovery,
            dns_discovery_updates,
        })
    }
}

impl<S, N> CloneDiscoveryHandle for Discovery<Discv5BCv4, S, N> {
    fn handle(&self) -> Option<DiscoveryHandle> {
        Some(DiscoveryHandle::V5BCv4(self.disc.as_ref()?.clone()))
    }
}

impl<S, T> Stream for Discovery<Discv5BCv4<T>, S, Enr<SecretKey>>
where
    S: Stream<Item = reth_discv5::DiscoveryUpdate> + Unpin + Send + 'static,
    T: FilterDiscovered + Unpin,
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
                reth_discv5::DiscoveryUpdate::V4(update) => {
                    let mut count_discovered = 0;
                    match &update {
                        reth_discv4::DiscoveryUpdate::Added(_) |
                        reth_discv4::DiscoveryUpdate::DiscoveredAtCapacity(_) => {
                            count_discovered += 1
                        }
                        reth_discv4::DiscoveryUpdate::Batch(updates) => {
                            for update in updates {
                                match update {
                                    reth_discv4::DiscoveryUpdate::Added(_) |
                                    reth_discv4::DiscoveryUpdate::DiscoveredAtCapacity(_) => {
                                        count_discovered += 1
                                    }
                                    _ => (),
                                }
                            }
                        }
                        _ => (),
                    }

                    let discv5 = self.disc.as_mut().unwrap();
                    discv5.with_metrics(|metrics| {
                        metrics
                            .discovered_peers_by_protocol
                            .increment_discovered_v4_as_downgrade(count_discovered)
                    });

                    self.on_discv4_update(update)
                }
                reth_discv5::DiscoveryUpdate::V5(update) => self.on_discv5_update(update),
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
    use rand::thread_rng;
    use reth_discv4::{DiscoveryUpdate, Discv4ConfigBuilder};
    use reth_discv5::{enr::EnrCombinedKeyWrapper, filter::NoopFilter, HandleDiscv5};
    use reth_net_common::discovery::HandleDiscovery;
    use tracing::trace;

    use super::*;

    async fn start_discovery_node(
        udp_port_discv4: u16,
        udp_port_discv5: u16,
    ) -> Discovery<Discv5BCv4<NoopFilter>, MergedUpdateStream, enr::Enr<secp256k1::SecretKey>> {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv4_addr = format!("127.0.0.1:{udp_port_discv4}").parse().unwrap();
        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        // disable `NatResolver`
        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = reth_discv5::Config::builder()
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .filter(NoopFilter)
            .build();

        Discovery::start_discv5_with_v4_downgrade(
            discv4_addr,
            secret_key,
            discv4_config,
            discv5_config,
            None,
        )
        .await
        .expect("should build discv5 with discv4 downgrade")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn discv5_with_discv4_downgrade() {
        reth_tracing::init_test_tracing();

        let mut node_1 = start_discovery_node(40014, 40015).await;
        let discv4_enr_1 = node_1.disc.as_ref().unwrap().with_discv4(|discv4| discv4.node_record());
        let discv5_enr_node_1 =
            node_1.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let discv4_id_1 = discv4_enr_1.id;
        let discv5_id_1 = discv5_enr_node_1.node_id();

        let mut node_2 = start_discovery_node(40024, 40025).await;
        let discv4_enr_2 = node_2.disc.as_ref().unwrap().with_discv4(|discv4| discv4.node_record());
        let discv5_enr_node_2 =
            node_2.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let discv4_id_2 = discv4_enr_2.id;
        let discv5_id_2 = discv5_enr_node_2.node_id();

        trace!(target: "net::discovery::discv5_downgrade_v4::tests",
            node_1_node_id=format!("{:#}", discv5_id_1),
            node_2_node_id=format!("{:#}", discv5_id_2),
            "started nodes"
        );

        // add node_2 manually to node_1:discv4 kbuckets
        node_1.disc.as_ref().unwrap().with_discv4(|discv4| {
            _ = discv4.add_node_to_routing_table(NodeFromExternalSource::NodeRecord(discv4_enr_2));
        });

        // verify node_2 is in KBuckets of node_1:discv4 and vv
        let event_1_v4 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_2_v4 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v4, reth_discv5::DiscoveryUpdate::V4(DiscoveryUpdate::Added(node)) if node == discv4_enr_2);
        matches!(event_2_v4, reth_discv5::DiscoveryUpdate::V4(DiscoveryUpdate::Added(node)) if node == discv4_enr_1);

        // add node_2 to discovery handle of node_1 (should add node to discv5 kbuckets)
        let discv5_enr_node_2_reth_compatible_ty: Enr<SecretKey> =
            EnrCombinedKeyWrapper(discv5_enr_node_2.clone()).into();
        node_1
            .disc
            .as_ref()
            .unwrap()
            .add_node_to_routing_table(NodeFromExternalSource::Enr(
                discv5_enr_node_2_reth_compatible_ty,
            ))
            .unwrap();
        // verify node_2 is in KBuckets of node_1:discv5
        assert!(node_1
            .disc
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.table_entries_id().contains(&discv5_id_2)));

        // manually trigger connection from node_1 to node_2
        node_1
            .disc
            .as_ref()
            .unwrap()
            .with_discv5(|discv5| discv5.send_ping(discv5_enr_node_2.clone()))
            .await
            .unwrap();

        // verify node_1:discv5 is connected to node_2:discv5 and vv
        let event_2_v5 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_1_v5 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v5, reth_discv5::DiscoveryUpdate::V5(discv5::Event::SessionEstablished(node, socket)) if node == discv5_enr_node_2 && socket == discv5_enr_node_2.udp4_socket().unwrap().into());
        matches!(event_2_v5, reth_discv5::DiscoveryUpdate::V5(discv5::Event::SessionEstablished(node, socket)) if node == discv5_enr_node_1 && socket == discv5_enr_node_1.udp4_socket().unwrap().into());

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_2_v5, reth_discv5::DiscoveryUpdate::V5(discv5::Event::NodeInserted { node_id, replaced }) if node_id == discv5_id_2 && replaced.is_none());

        let event_2_v4 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_1_v4 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v4, reth_discv5::DiscoveryUpdate::V4(DiscoveryUpdate::Removed(node_id)) if node_id == discv4_id_2);
        matches!(event_2_v4, reth_discv5::DiscoveryUpdate::V4(DiscoveryUpdate::Removed(node_id)) if node_id == discv4_id_1);
    }
}
