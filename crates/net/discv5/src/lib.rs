//! Wrapper around [`discv5::Discv5`].

use std::{fmt, net::IpAddr, sync::Arc};

use ::enr::Enr;
use alloy_rlp::Decodable;
use derive_more::{Constructor, Deref, DerefMut};
use discv5::IpMode;
use enr::{uncompressed_to_compressed_id, EnrCombinedKeyWrapper};
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    ForkId, NodeRecord, PeerId,
};
use tracing::error;

pub mod config;
pub mod discv5_downgrade_v4;
pub mod enr;

pub use config::{BootNode, DiscV5Config, DiscV5ConfigBuilder};
pub use discv5_downgrade_v4::{DiscV5WithV4Downgrade, MergedUpdateStream};
pub use enr::uncompressed_id_from_enr_pk;

/// Errors from using [`discv5::Discv5`] handle.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failure adding node to [`discv5::Discv5`].
    #[error("failed adding node to discv5, {0}")]
    AddNodeToDiscv5Failed(&'static str),
    /// Missing key used to identify mempool network.
    #[error("fork id missing on enr, 'eth' key missing")]
    ForkIdMissing,
    /// Failed to decode [`ForkId`] rlp value.
    #[error("failed to decode fork id, 'eth': {0:?}")]
    ForkIdDecodeError(#[from] alloy_rlp::Error),
    /// Peer is unreachable over discovery.
    #[error("discovery socket missing, ENR: {0}")]
    UnreachableDiscovery(discv5::Enr),
    /// Peer is unreachable over mempool.
    #[error("mempool TCP socket missing, ENR: {0}")]
    UnreachableMempool(discv5::Enr),
    /// Peer is not using same IP version as local node in discovery.
    #[error("discovery socket is unsupported IP version, ENR: {0}, local ip mode: {1:?}")]
    IpVersionMismatchDiscovery(discv5::Enr, IpMode),
    /// Peer is not using same IP version as local node in mempool.
    #[error("mempool TCP socket is unsupported IP version, ENR: {0}, local ip mode: {1:?}")]
    IpVersionMismatchMempool(discv5::Enr, IpMode),
}

/// Use API of [`discv5::Discv5`].
pub trait HandleDiscv5 {
    /// Exposes API of [`discv5::Discv5`].
    fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DiscV5) -> R;

    /// Returns the [`IpMode`] of the local node.
    fn ip_mode(&self) -> IpMode;

    /// Returns the [`ForkId`] of the given [`Enr`](discv5::Enr), if field is set.
    fn get_fork_id<K: discv5::enr::EnrKey>(
        &self,
        enr: &discv5::enr::Enr<K>,
    ) -> Result<ForkId, Error> {
        let mut fork_id_bytes = enr.get(b"eth").ok_or(Error::ForkIdMissing)?;

        Ok(ForkId::decode(&mut fork_id_bytes)?)
    }

    /// Tries to convert an [`Enr`](discv5::Enr) into the backwards compatible type [`NodeRecord`],
    /// w.r.t. local [`IpMode`].
    fn try_into_reachable(&self, enr: discv5::Enr) -> Result<NodeRecord, Error> {
        // todo: track unreachable with metrics
        if enr.udp4_socket().is_none() && enr.udp6_socket().is_none() {
            return Err(Error::UnreachableDiscovery(enr))
        }
        let Some(udp_socket) = self.ip_mode().get_contactable_addr(&enr) else {
            return Err(Error::IpVersionMismatchDiscovery(enr, self.ip_mode()))
        };
        // since we, on bootstrap, set tcp4 in local ENR for `IpMode::Dual`, we prefer tcp4 here
        // too
        let Some(tcp_port) = (match self.ip_mode() {
            IpMode::Ip4 | IpMode::DualStack => enr.tcp4(),
            IpMode::Ip6 => enr.tcp6(),
        }) else {
            return Err(Error::IpVersionMismatchMempool(enr, self.ip_mode()))
        };

        let id = uncompressed_id_from_enr_pk(&enr);

        Ok(NodeRecord { address: udp_socket.ip(), tcp_port, udp_port: udp_socket.port(), id })
    }
}

/// Transparent wrapper around [`discv5::Discv5`].
#[derive(Deref, DerefMut, Clone, Constructor)]
pub struct DiscV5 {
    #[deref]
    #[deref_mut]
    discv5: Arc<discv5::Discv5>,
    ip_mode: IpMode,
    // Notify app of discovered nodes that don't have a TCP port set in their ENR. These nodes are
    // filtered out by default. allow_no_tcp_discovered_nodes: bool,
}

impl DiscV5 {
    fn add_node(&self, node_record: NodeFromExternalSource) -> Result<(), Error> {
        let NodeFromExternalSource::Enr(enr) = node_record else {
            unreachable!("cannot convert `NodeRecord` type to `Enr` type")
        };
        let enr = enr.into();
        let EnrCombinedKeyWrapper(enr) = enr;
        self.add_enr(enr).map_err(Error::AddNodeToDiscv5Failed)
    }

    fn update_local_enr(&self, key: &[u8], rlp: &Bytes) {
        let Ok(key_str) = std::str::from_utf8(key) else {
            error!(target: "discv5",
                err="key not utf-8",
                "failed to update local enr"
            );
            return
        };
        if let Err(err) = self.enr_insert(key_str, rlp) {
            error!(target: "discv5",
                %err,
                "failed to update local enr"
            );
        }
    }
}

impl fmt::Debug for DiscV5 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "{ .. }".fmt(f)
    }
}

impl HandleDiscovery for DiscV5 {
    fn add_node_to_routing_table(
        &self,
        node_record: NodeFromExternalSource,
    ) -> Result<(), impl std::error::Error> {
        self.add_node(node_record)
    }

    fn set_eip868_in_local_enr(&self, key: Vec<u8>, rlp: Bytes) {
        self.update_local_enr(&key, &rlp)
    }

    fn encode_and_set_eip868_in_local_enr(&self, key: Vec<u8>, value: impl alloy_rlp::Encodable) {
        let mut buf = BytesMut::new();
        value.encode(&mut buf);
        self.set_eip868_in_local_enr(key, buf.freeze())
    }

    fn ban_peer_by_ip_and_node_id(&self, peer_id: PeerId, ip: IpAddr) {
        let node_id = uncompressed_to_compressed_id(peer_id);
        self.ban_node(&node_id, None);
        self.ban_peer_by_ip(ip);
    }

    fn ban_peer_by_ip(&self, ip: IpAddr) {
        self.ban_ip(ip, None);
    }

    fn node_record(&self) -> NodeRecord {
        let enr: Enr<_> = EnrCombinedKeyWrapper(self.local_enr()).into();
        enr.try_into().unwrap()
    }
}

impl HandleDiscv5 for DiscV5 {
    fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DiscV5) -> R,
    {
        f(self)
    }

    fn ip_mode(&self) -> IpMode {
        self.ip_mode
    }
}
