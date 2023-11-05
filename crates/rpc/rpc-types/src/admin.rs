use crate::{NodeRecord, PeerId};
use alloy_primitives::{B256, U256};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
};

/// Represents the `admin_nodeInfo` response, which can be queried for all the information
/// known about the running node at the networking granularity.
///
/// Note: this format is not standardized. Reth follows Geth's format,
/// see: <https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-admin>
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Enode of the node in URL format.
    pub enode: NodeRecord,
    /// ID of the local node.
    pub id: PeerId,
    /// IP of the local node.
    pub ip: IpAddr,
    /// Address exposed for listening for the local node.
    #[serde(rename = "listenAddr")]
    pub listen_addr: SocketAddr,
    /// Ports exposed by the node for discovery and listening.
    pub ports: Ports,
    /// Name of the network
    pub name: String,
    /// Networking protocols being run by the local node.
    pub protocols: Protocols,
}

impl NodeInfo {
    /// Creates a new instance of `NodeInfo`.
    pub fn new(enr: NodeRecord, status: NetworkStatus) -> NodeInfo {
        NodeInfo {
            enode: enr,
            id: enr.id,
            ip: enr.address,
            listen_addr: enr.tcp_addr(),
            ports: Ports { discovery: enr.udp_port, listener: enr.tcp_port },
            name: status.client_version,
            protocols: Protocols { eth: status.eth_protocol_info, other: Default::default() },
        }
    }
}

/// All supported protocols
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocols {
    /// Info about `eth` sub-protocol
    pub eth: EthProtocolInfo,
    /// Placeholder for any other protocols
    #[serde(flatten, default)]
    pub other: BTreeMap<String, serde_json::Value>,
}

/// Ports exposed by the node for discovery and listening.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ports {
    /// Port exposed for node discovery.
    pub discovery: u16,
    /// Port exposed for listening.
    pub listener: u16,
}

/// The status of the network being ran by the local node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkStatus {
    /// The local node client version.
    pub client_version: String,
    /// The current ethereum protocol version
    pub protocol_version: u64,
    /// Information about the Ethereum Wire Protocol.
    pub eth_protocol_info: EthProtocolInfo,
}

/// Information about the Ethereum Wire Protocol (ETH)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EthProtocolInfo {
    /// The current difficulty at the head of the chain.
    #[serde(deserialize_with = "crate::serde_helpers::json_u256::deserialize_json_u256")]
    pub difficulty: U256,
    /// The block hash of the head of the chain.
    pub head: B256,
    /// Network ID in base 10.
    pub network: u64,
    /// Genesis block of the current chain.
    pub genesis: B256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_node_info_roundtrip() {
        let sample = r#"{"enode":"enode://44826a5d6a55f88a18298bca4773fca5749cdc3a5c9f308aa7d810e9b31123f3e7c5fba0b1d70aac5308426f47df2a128a6747040a3815cc7dd7167d03be320d@[::]:30303","id":"44826a5d6a55f88a18298bca4773fca5749cdc3a5c9f308aa7d810e9b31123f3e7c5fba0b1d70aac5308426f47df2a128a6747040a3815cc7dd7167d03be320d","ip":"::","listenAddr":"[::]:30303","name":"reth","ports":{"discovery":30303,"listener":30303},"protocols":{"eth":{"difficulty":17334254859343145000,"genesis":"0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3","head":"0xb83f73fbe6220c111136aefd27b160bf4a34085c65ba89f24246b3162257c36a","network":1}}}"#;

        let info: NodeInfo = serde_json::from_str(sample).unwrap();
        let serialized = serde_json::to_string_pretty(&info).unwrap();
        let de_serialized: NodeInfo = serde_json::from_str(&serialized).unwrap();
        assert_eq!(info, de_serialized)
    }
}
