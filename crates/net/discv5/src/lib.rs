//! Wrapper around [`discv5::Discv5`].

use std::{fmt, net::IpAddr, sync::Arc};

use ::enr::Enr;
use derive_more::{Constructor, Deref, DerefMut};
use discv5::IpMode;
use enr::{uncompressed_to_compressed_id, EnrCombinedKeyWrapper};
use reth_net_common::discovery::{HandleDiscovery, NodeFromExternalSource};
use reth_primitives::{
    bytes::{Bytes, BytesMut},
    NodeRecord, PeerId,
};
use tracing::error;

pub mod config;
pub mod discv5_downgrade_v4;
pub mod enr;

pub use config::{DiscV5Config, DiscV5ConfigBuilder};
pub use discv5_downgrade_v4::{DiscV5WithV4Downgrade, MergedUpdateStream};

/// Errors from using [`discv5::Discv5`] handle.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Failure adding node to [`discv5::Discv5`].
    #[error("failed adding node to discv5, {0}")]
    AddNodeToDiscv5Failed(&'static str),
}

/// Use API of [`discv5::Discv5`].
pub trait HandleDiscv5 {
    /// Exposes API of [`discv5::Discv5`].
    fn with_discv5<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&DiscV5) -> R;

    /// Returns the [`IpMode`] of the local node.
    fn ip_mode(&self) -> IpMode;
}

/// Transparent wrapper around [`discv5::Discv5`].
#[derive(Deref, DerefMut, Clone, Constructor)]
pub struct DiscV5 {
    #[deref]
    #[deref_mut]
    discv5: Arc<discv5::Discv5>,
    ip_mode: IpMode,
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

    fn enr_insert_fork_id(&self, key: &[u8], rlp: &Bytes) {
        let Ok(key_str) = std::str::from_utf8(key) else {
            error!(target: "discv5", "fork id should be utf-8");
            return
        };
        if let Err(err) = self.enr_insert(key_str, rlp) {
            error!(target: "discv5",
                %err,
                "failed to update discv5 enr"
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
        self.enr_insert_fork_id(&key, &rlp)
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
