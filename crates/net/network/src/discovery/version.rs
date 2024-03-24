//! Wrapper around versioned discovery types.

use std::{
    error::Error,
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use derive_more::From;
use futures::{Stream, StreamExt};
use pin_project::pin_project;
use reth_discv4::{secp256k1::SecretKey, Discv4, Discv4Config};
use reth_discv5::{Discv5, Discv5BCv4, FilterDiscovered, MustIncludeChain};
use reth_dns_discovery::DnsDiscoveryConfig;
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::{ForkId, PeerId};
use tokio::sync::mpsc;

use crate::{error::NetworkError, DiscoveryEvent};

use super::{v5::Discovery5, v5_downgrade_v4::Discovery5BC, Discovery as Discovery4};

/// Used by [`NetworkManager`](crate::NetworkManager) to interface with versioned [`Discovery`]
/// wrapper.
pub trait HandleDiscoveryServices {
    /// Updates the `eth:ForkId` field in discovery.
    fn update_fork_id(&self, fork_id: ForkId);

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    fn ban(&self, peer_id: PeerId, ip: IpAddr);

    /// Bans the [`IpAddr`] in the discovery service.
    fn ban_ip(&self, ip: IpAddr);

    /// Returns the id with which the local identifies itself in the network
    fn local_id(&self) -> PeerId;

    /// Registers a listener for receiving [DiscoveryEvent] updates.
    fn add_listener(&mut self, tx: mpsc::UnboundedSender<DiscoveryEvent>);
}

/// Returns a versioned wrapper around the discovery handle.
pub trait CloneDiscoveryHandle {
    /// Returns a thread safe shared reference to the discovery handle.
    fn handle(&self) -> Option<DiscoveryHandle>;
}

/// Implements body of trait methods, for traits which are implemented for each type in variants.
macro_rules! trait_method_body {
    ($self:ident, $f:ident, $($params:ident,)* $(.$map:ident($err:expr))?) => {
        match $self {
            Self::V5(discv5) => discv5.$f($($params,)*)$(.$map($err))?,
            Self::V5BCv4(discv5) => discv5.$f($($params,)*)$(.$map($err))?,
            Self::V4(discv4) => discv4.$f($($params,)*)$(.$map($err))?,
        }
    };
}

/// Devp2p discovery node. Transparent wrapper of versioned types.
#[pin_project(project = EnumProj)]
#[derive(Debug, From)]
pub enum Discovery<T = MustIncludeChain> {
    /// Protocol version 5.
    V5(#[pin] Discovery5<T>),
    /// Protocol version 5, with support for version 4 backwards compatibility.
    V5BCv4(#[pin] Discovery5BC<T>),
    /// Protocol version 4.
    V4(#[pin] Discovery4),
}

impl<T> Discovery<T> {
    /// Starts [`discv5::Discv5`].
    pub async fn start(
        discv4_addr: std::net::SocketAddr,
        sk: SecretKey,
        discv4_config: Option<Discv4Config>,
        discv5_config: Option<reth_discv5::Config<T>>,
        dns_discovery_config: Option<DnsDiscoveryConfig>,
    ) -> Result<Self, NetworkError>
    where
        T: FilterDiscovered + Send + Sync + Clone + 'static,
    {
        Ok(if discv5_config.is_none() {
            Self::V4(Discovery4::new(discv4_addr, sk, discv4_config, dns_discovery_config).await?)
        } else if let Some(v4_config) = discv4_config {
            Self::V5BCv4(
                Discovery5BC::<T>::start_discv5_with_v4_downgrade(
                    discv4_addr,
                    sk,
                    v4_config,
                    discv5_config.unwrap(),
                    dns_discovery_config,
                )
                .await?,
            )
        } else {
            Self::V5(
                Discovery5::<T>::start_discv5(sk, discv5_config.unwrap(), dns_discovery_config)
                    .await?,
            )
        })
    }
}

impl HandleDiscoveryServices for Discovery {
    fn update_fork_id(&self, fork_id: ForkId) {
        trait_method_body!(self, update_fork_id, fork_id,)
    }

    fn ban(&self, peer_id: PeerId, ip: IpAddr) {
        trait_method_body!(self, ban, peer_id, ip,)
    }

    fn ban_ip(&self, ip: IpAddr) {
        trait_method_body!(self, ban_ip, ip,)
    }

    fn local_id(&self) -> PeerId {
        trait_method_body!(self, local_id,)
    }

    fn add_listener(&mut self, tx: mpsc::UnboundedSender<DiscoveryEvent>) {
        match self {
            Self::V5(discv5) => discv5.discovery_listeners.push(tx),
            Self::V5BCv4(discv5) => discv5.discovery_listeners.push(tx),
            Self::V4(discv4) => discv4.discovery_listeners.push(tx),
        }
    }
}

impl CloneDiscoveryHandle for Discovery {
    fn handle(&self) -> Option<DiscoveryHandle> {
        trait_method_body!(self, handle,)
    }
}

impl Stream for Discovery {
    type Item = DiscoveryEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.project() {
            EnumProj::V5(mut discv5) => discv5.poll_next_unpin(cx),
            EnumProj::V5BCv4(mut discv5) => discv5.poll_next_unpin(cx),
            EnumProj::V4(mut discv4) => discv4.poll_next_unpin(cx),
        }
    }
}

/// Handle to devp2p discovery node. Transparent wrapper of versioned types.
#[derive(Debug, From)]
pub enum DiscoveryHandle {
    /// Protocol version 5.
    V5(Discv5),
    /// Protocol version 5, with support for version 4 backwards compatibility.
    V5BCv4(Discv5BCv4),
    /// Protocol version 4.
    V4(Discv4),
}

impl HandleDiscovery for DiscoveryHandle {
    fn add_node_to_routing_table(
        &self,
        node_record: NodeFromExternalSource,
    ) -> Result<(), impl std::error::Error> {
        trait_method_body!(self, add_node_to_routing_table, node_record, .map_err(|err| Arc::new(err) as Arc<dyn Error> ))
    }

    fn ban_peer_by_ip(&self, ip: IpAddr) {
        trait_method_body!(self, ban_peer_by_ip, ip,)
    }

    fn ban_peer_by_ip_and_node_id(&self, node_id: PeerId, ip: IpAddr) {
        trait_method_body!(self, ban_peer_by_ip_and_node_id, node_id, ip,)
    }

    fn encode_and_set_eip868_in_local_enr(&self, key: Vec<u8>, value: impl alloy_rlp::Encodable) {
        trait_method_body!(self, encode_and_set_eip868_in_local_enr, key, value,)
    }

    fn set_eip868_in_local_enr(&self, key: Vec<u8>, rlp: alloy_rlp::Bytes) {
        trait_method_body!(self, set_eip868_in_local_enr, key, rlp,);
    }

    fn node_record(&self) -> reth_discv4::NodeRecord {
        trait_method_body!(self, node_record,)
    }
}
