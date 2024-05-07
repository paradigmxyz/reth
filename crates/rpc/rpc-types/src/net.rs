use alloy_rpc_types::admin::EthProtocolInfo;
use serde::{Deserialize, Serialize};

/// The status of the network being ran by the local node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStatus {
    /// The local node client version.
    pub client_version: String,
    /// The current ethereum protocol version
    pub protocol_version: u64,
    /// Information about the Ethereum Wire Protocol.
    pub eth_protocol_info: EthProtocolInfo,
}
