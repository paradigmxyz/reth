use reth_primitives::{NodeRecord, PeerId, H256, U256};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
};

/// Represents the `admin_nodeInfo` response, which can be queried for all the information
/// known about the running node at the networking granularity.
///
/// Note: this format is not standardized.  Reth follows geth's format,
/// see: https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-admin
#[derive(Serialize, Deserialize, Debug)]
pub struct NodeInfo {
    /// Enode in URL format.
    pub enode: NodeRecord,
    /// ID of the local node.
    pub id: PeerId,
    /// IP of the local node.
    pub ip: IpAddr,
    /// Address exposed for listening for the local node.
    #[serde(rename = "listenAddr")]
    pub listen_addr: SocketAddr,
    /// Local node client name.
    pub name: String,
    /// Ports exposed by the node for discovery and listening.
    pub ports: Ports,
    /// Networking protocols being run by the local node.
    pub protocols: BTreeMap<String, ProtocolInfo>,
}

impl NodeInfo {
    /// Creates a new instance of `NodeInfo`.
    pub fn new(enr: NodeRecord) -> NodeInfo {
        let protocol_info =
            BTreeMap::from([("eth".into(), ProtocolInfo::Eth(EthProtocolInfo::default()))]);

        NodeInfo {
            enode: enr,
            id: enr.id,
            ip: enr.address,
            listen_addr: enr.tcp_addr(),
            name: "Reth".to_owned(),
            ports: Ports { discovery: enr.udp_port, listener: enr.tcp_port },
            protocols: protocol_info,
        }
    }
}

/// Ports exposed by the node for discovery and listening.
#[derive(Serialize, Deserialize, Debug)]
pub struct Ports {
    /// Port exposed for node discovery.
    pub discovery: u16,
    /// Port exposed for listening.
    pub listener: u16,
}

/// Information about the different protocols that can be run by the node (ETH, )
#[derive(Serialize, Deserialize, Debug)]
pub enum ProtocolInfo {
    /// Information about the Ethereum Wire Protocol.
    Eth(EthProtocolInfo),
}
/// Information about the Ethereum Wire Protocol (ETH)
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct EthProtocolInfo {
    /// The current difficulty at the head of the chain.
    pub difficulty: U256,
    /// The block hash of the head of the chain.
    pub head: H256,
    /// Network ID in base 10.
    pub network: u64,
    /// Genesis block of the current chain.
    pub genesis: H256,
}
