use reth_network_api::NetworkStatus;
use reth_primitives::{NodeRecord, PeerId};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};

/// Represents the `admin_nodeInfo` response, which can be queried for all the information
/// known about the running node at the networking granularity.
///
/// Note: this format is not standardized. Reth follows Geth's format,
/// see: <https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-admin>
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
    /// Ports exposed by the node for discovery and listening.
    pub ports: Ports,
    /// Networking protocols being run by the local node.
    #[serde(flatten)]
    pub status: NetworkStatus,
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
            status,
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
