use std::net::{IpAddr, SocketAddr};
use reth_primitives::{NodeRecord, U256, H512, rpc::H256};
use serde::{Deserialize, Serialize};

/// Represents the `admin_nodeInfo` response, which can be queried for all the information
/// known about the running node at the networking granularity.
///
/// Note: this format is not standardized.  Reth follows geth's format,
/// see: https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-admin
#[derive(Serialize, Deserialize, Debug)]
pub struct NodeInfo {
    /// Enode in URL format.
    pub enode: String,
    /// ID of the local node.
    pub id: H512,
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
    pub protocols: Protocols,
}

impl NodeInfo {
    /// Creates a new instance of `NodeInfo`.
    pub fn new(enr: NodeRecord) -> NodeInfo {
        NodeInfo {
            enode: enr.to_string(),
            id: enr.id,
            ip: enr.address,
            listen_addr: enr.tcp_addr(),
            name: "Reth".to_owned(),
            ports: Ports { discovery: enr.tcp_port.into(), listener: enr.tcp_port.into() },
            protocols: Protocols {
                eth: Eth { difficulty: todo!(), genesis: todo!(), head: todo!(), network: todo!() },
            },
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

/// Networking protocols being run by the node.
#[derive(Serialize, Deserialize, Debug)]
pub struct Protocols {
    /// Information about the Ethereum Wire Protocol (ETH)
    pub eth: Eth,
}

/// Information about the Ethereum Wire Protocol (ETH)
#[derive(Serialize, Deserialize, Debug)]
pub struct Eth {
    /// Total difficulty of the best chain.
    pub difficulty: U256,
    /// The hash of the genesis block.
    pub genesis: H256,
    /// Hash of the latest block of the best chain.
    pub head: H256,
    /// Network ID in base 10.
    pub network: u64,
}
