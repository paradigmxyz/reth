//! Minimal interface between discovery and network.

use std::{error::Error, net::IpAddr, sync::Arc};

use derive_more::From;
use enr::Enr;
use reth_primitives::{bytes::Bytes, NodeRecord, PeerId};

#[derive(Debug, From)]
/// A node that hasn't been discovered via a discv5 or discv4 query.
pub enum NodeFromExternalSource {
    /// Node compatible with discv4 routing table.
    NodeRecord(NodeRecord),
    /// Node compatible with discv5 routing table.
    Enr(Enr<secp256k1::SecretKey>),
}

/// Essential interface for interacting with discovery.
pub trait HandleDiscovery {
    /// Adds the node to the table, if it is not already present.
    fn add_node_to_routing_table(
        &self,
        node_record: NodeFromExternalSource,
    ) -> Result<(), impl Error>;

    /// Sets the pair in the EIP-868 [`Enr`] of the node.
    ///
    /// If the key already exists, this will update it.
    ///
    /// CAUTION: The value **must** be rlp encoded
    fn set_eip868_in_local_enr(&self, key: Vec<u8>, rlp: Bytes);

    /// Sets the pair in the EIP-868 [`Enr`] of the node.
    ///
    /// If the key already exists, this will update it.
    fn encode_and_set_eip868_in_local_enr(&self, key: Vec<u8>, value: impl alloy_rlp::Encodable);

    /// Adds the peer and id to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    fn ban_peer_by_ip_and_node_id(&self, node_id: PeerId, ip: IpAddr);

    /// Adds the ip to the ban list.
    ///
    /// This will prevent any future inclusion in the table
    fn ban_peer_by_ip(&self, ip: IpAddr);

    /// Returns the [`NodeRecord`] of the local node.
    ///
    /// This includes the currently tracked external IP address of the node.
    fn node_record(&self) -> NodeRecord;
}
