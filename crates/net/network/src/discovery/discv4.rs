//! Discovery support using [`Discv4`].
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

use futures::StreamExt;
use reth_discv4::{secp256k1::SecretKey, Discv4, Discv4Config};
use reth_dns_discovery::{new_with_dns_resolver, DnsDiscoveryConfig};
use reth_net_common::discovery::NodeFromExternalSource;
use reth_primitives::NodeRecord;
use tokio_stream::Stream;
use tracing::trace;

use crate::error::{NetworkError, ServiceKind};

use super::{Discovery, DiscoveryEvent};

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
        trace!(target: "net::discovery::discv4",
            "starting discovery .."
        );

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
                new_with_dns_resolver::<NodeRecord>(dns_config)?
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

    /// Returns a shared reference to the [`Discv4`] handle.
    pub fn discv4(&self) -> Option<Discv4> {
        self.disc.clone()
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

        if self.queued_events.is_empty() {
            cx.waker().wake_by_ref();
        }

        Poll::Pending
    }
}
