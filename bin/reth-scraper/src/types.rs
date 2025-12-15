use alloy_primitives::B512;
use std::net::IpAddr;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeInfo {
    pub node_id: B512,
    pub enode: String,
    pub ip: IpAddr,
    pub tcp_port: u16,
    pub udp_port: u16,
    pub client_version: String,
    pub capabilities: Vec<String>,
    pub eth_version: Option<u8>,
    pub chain_id: Option<u64>,
    pub country_code: Option<String>,
    pub first_seen: u64,
    pub last_seen: u64,
    pub last_error: Option<String>,
    pub last_checked: Option<u64>,
    pub is_alive: bool,
    pub consecutive_failures: u32,
}

impl NodeInfo {
    pub fn capabilities_string(&self) -> String {
        self.capabilities.join(",")
    }
}
