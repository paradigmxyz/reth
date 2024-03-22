//! Discovery support for the network using [`discv5::Discv5`], with support for downgraded
//! [`Discv4`] connections.

use std::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

use futures::StreamExt;
use reth_discv4::{secp256k1::SecretKey, Discv4Config};
use reth_discv5::{
    discv5::enr::Enr, downgrade_v4::DiscoveryUpdateV5, filter::FilterDiscovered, DiscV5Config,
    DiscV5WithV4Downgrade, MergedUpdateStream,
};
use reth_dns_discovery::{new_with_dns_resolver, DnsDiscoveryConfig};
use reth_net_common::discovery::NodeFromExternalSource;
use reth_primitives::NodeRecord;

use tokio_stream::Stream;
use tracing::{error, trace};

use crate::error::NetworkError;

use super::{Discovery, DiscoveryEvent};

/// [`Discovery`] type that uses [`discv5::Discv5`], with support for downgraded [`Discv4`]
/// connections.
#[cfg(feature = "discv5-downgrade-v4")]
pub type DiscoveryV5V4<T> = Discovery<DiscV5WithV4Downgrade<T>, MergedUpdateStream, Enr<SecretKey>>;

impl<T> Discovery<DiscV5WithV4Downgrade<T>, MergedUpdateStream, Enr<SecretKey>> {
    /// Spawns the discovery service.
    ///
    /// This will spawn [`discv5::Discv5`] and [`Discv4`] each onto their own new task and
    /// establish a merged listener channel to receive all discovered nodes.
    pub async fn start_discv5_with_v4_downgrade(
        discv4_addr: SocketAddr, // discv5 addr in config
        sk: SecretKey,
        discv4_config: Option<Discv4Config>,
        discv5_config: Option<DiscV5Config<T>>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError>
    where
        T: FilterDiscovered + Clone + Send + Sync + 'static,
    {
        trace!(target: "net::discovery::discv5_downgrade_v4",
            "starting discovery .."
        );

        let (disc, disc_updates, bc_local_discv5_enr) = match (discv4_config, discv5_config) {
            (Some(discv4_config), Some(discv5_config)) => {
                let (disc, disc_updates, bc_discv5_enr) =
                    DiscV5WithV4Downgrade::start(discv4_addr, sk, discv4_config, discv5_config)
                        .await
                        .map_err(|err| NetworkError::custom_discovery(&err.to_string()))?;

                (Some(disc), Some(disc_updates), bc_discv5_enr)
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
                new_with_dns_resolver::<Enr<SecretKey>>(dns_config)?
            } else {
                (None, None, None)
            };

        Ok(Discovery {
            discovery_listeners: Default::default(),
            local_enr: bc_local_discv5_enr,
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

#[cfg(feature = "discv5-downgrade-v4")]
impl Discovery<DiscV5WithV4Downgrade<T>, MergedUpdateStream, Enr<SecretKey>> {
    pub async fn start(
        discv4_addr: SocketAddr,
        sk: SecretKey,
        discv4_config: Option<reth_discv4::Discv4Config>,
        discv5_config: Option<DiscV5Config<T>>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError>
    where
        T: FilterDiscovered + Send + Sync + Clone + 'static,
    {
        Discovery::start_discv5_with_v4_downgrade(
            discv4_addr,
            sk,
            discv4_config,
            discv5_config,
            dns_discovery_config,
        )
        .await
    }

    /// Returns a shared reference to the [`DiscV5WithV4Downgrade`] handle.
    pub fn discv5(&self) -> Option<DiscV5WithV4Downgrade> {
        self.disc.clone()
    }
}

impl<S, T> Stream for Discovery<DiscV5WithV4Downgrade<T>, S, Enr<SecretKey>>
where
    S: Stream<Item = DiscoveryUpdateV5> + Unpin + Send + 'static,
    T: FilterDiscovered,
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
                    if let Err(err) = self.on_discv5_update(update) {
                        error!(target: "net::discovery::discv5_downgrade_v4", %err, "failed to process update");
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
    use reth_discv5::{discv5, enr::EnrCombinedKeyWrapper, filter::NoopFilter, HandleDiscv5};
    use reth_net_common::discovery::HandleDiscovery;
    use tracing::trace;

    use super::*;

    async fn start_discovery_node(
        udp_port_discv4: u16,
        udp_port_discv5: u16,
    ) -> Discovery<
        DiscV5WithV4Downgrade<NoopFilter>,
        MergedUpdateStream,
        enr::Enr<secp256k1::SecretKey>,
    > {
        let secret_key = SecretKey::new(&mut thread_rng());

        let discv4_addr = format!("127.0.0.1:{udp_port_discv4}").parse().unwrap();
        let discv5_addr: SocketAddr = format!("127.0.0.1:{udp_port_discv5}").parse().unwrap();

        // disable `NatResolver`
        let discv4_config = Discv4ConfigBuilder::default().external_ip_resolver(None).build();

        let discv5_listen_config = discv5::ListenConfig::from(discv5_addr);
        let discv5_config = DiscV5Config::builder()
            .discv5_config(discv5::ConfigBuilder::new(discv5_listen_config).build())
            .build();

        Discovery::start_discv5_with_v4_downgrade(
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
    async fn discv5_with_discv4_downgrade() {
        reth_tracing::init_test_tracing();

        let mut node_1 = start_discovery_node(31314, 31324).await;
        let discv4_enr_1 = node_1.disc.as_ref().unwrap().with_discv4(|discv4| discv4.node_record());
        let discv5_enr_node_1 =
            node_1.disc.as_ref().unwrap().with_discv5(|discv5| discv5.local_enr());
        let discv4_id_1 = discv4_enr_1.id;
        let discv5_id_1 = discv5_enr_node_1.node_id();

        let mut node_2 = start_discovery_node(32324, 32325).await;
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
        matches!(event_1_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Added(node)) if node == discv4_enr_2);
        matches!(event_2_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Added(node)) if node == discv4_enr_1);

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
        matches!(event_1_v5, DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(node, socket)) if node == discv5_enr_node_2 && socket == discv5_enr_node_2.udp4_socket().unwrap().into());
        matches!(event_2_v5, DiscoveryUpdateV5::V5(discv5::Event::SessionEstablished(node, socket)) if node == discv5_enr_node_1 && socket == discv5_enr_node_1.udp4_socket().unwrap().into());

        // verify node_1 is in KBuckets of node_2:discv5
        let event_2_v5 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_2_v5, DiscoveryUpdateV5::V5(discv5::Event::NodeInserted { node_id, replaced }) if node_id == discv5_id_2 && replaced.is_none());

        let event_2_v4 = node_2.disc_updates.as_mut().unwrap().next().await.unwrap();
        let event_1_v4 = node_1.disc_updates.as_mut().unwrap().next().await.unwrap();
        matches!(event_1_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Removed(node_id)) if node_id == discv4_id_2);
        matches!(event_2_v4, DiscoveryUpdateV5::V4(DiscoveryUpdate::Removed(node_id)) if node_id == discv4_id_1);
    }
}
