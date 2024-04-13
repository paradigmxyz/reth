#![allow(missing_docs)]
#![allow(unreachable_pub)]
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{
    Address, BlockHash, BlockId, BlockNumber, ChainId, Genesis, B256, U256, U64,
};
use reth_rpc_types::BlockNumberOrTag;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr, time::Duration};

/// todo: move to reth_rpc_types

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/id.go#L33
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L2BlockRef {
    pub hash: BlockHash,
    pub number: BlockNumber,
    pub parent_hash: BlockHash,
    pub timestamp: U64,
    pub l1origin: BlockId,
    pub sequence_number: u64,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/id.go#L52
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1BlockRef {
    pub hash: BlockHash,
    pub number: BlockNumber,
    pub parent_hash: BlockHash,
    pub timestamp: U64,
}

// #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
// pub struct BlockDescriptor {
//     pub hash: BlockHash,
//     pub number: U256,
//     pub parent_hash: BlockHash,
//     pub timestamp: U64,
// }

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/sync_status.go#L5
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    current_l1: L1BlockRef,
    current_l1_finalized: L1BlockRef,
    head_l1: L1BlockRef,
    safe_l1: L1BlockRef,
    finalized_l1: L1BlockRef,
    unsafe_l2: L2BlockRef,
    safe_l2: L2BlockRef,
    finalized_l2: L2BlockRef,
    pending_safe_l2: L2BlockRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputResponse {
    pub version: B256,
    pub output_root: B256,
    pub block_ref: L2BlockRef,
    pub withdrawal_storage_root: B256,
    pub state_root: B256,
    pub sync_status: SyncStatus,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/rollup/types.go#L53
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollupConfig {
    pub genesis: Genesis,
    pub block_time: u64,
    pub max_sequencer_drift: u64,
    pub seq_window_size: u64,
    pub channel_timeout: u64,
    pub l1_chain_id: Option<U256>,
    pub l2_chain_id: Option<U256>,

    pub regolith_time: Option<u64>,
    pub canyon_time: Option<u64>,
    pub delta_time: Option<u64>,
    pub ecotone_time: Option<u64>,
    pub fjord_time: Option<u64>,
    pub interop_time: Option<u64>,
    pub batch_inbox_address: Address,
    pub deposit_contract_address: Address,
    pub l1_system_config_address: Address,
    pub protocol_versions_address: Option<Address>,
    pub da_challenge_address: Option<Address>,
    pub da_challenge_window: u64,
    pub da_resolve_window: u64,
    pub use_plasma: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerConnectedness {
    NotConnected,
    Connected,
    CanConnect,
    CannotConnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Direction {
    Unknown,
    Inbound,
    Outbound,
}

/// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L13
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicScores {
    pub time_in_mesh: f64,
    pub first_message_deliveries: f64,
    pub mesh_message_deliveries: f64,
    pub invalid_message_deliveries: f64,
}

/// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L20C6-L20C18
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GossipScores {
    pub total: f64,
    pub blocks: TopicScores,
    #[serde(rename = "IPColocationFactor")]
    pub ip_colocation_factor: f64,
    pub behavioral_penalty: f64,
}

/// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L31C1-L35C2
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReqRespScores {
    pub valid_responses: f64,
    pub error_responses: f64,
    pub rejected_payloads: f64,
}

/// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L81
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerScores {
    pub gossip: GossipScores,
    pub req_resp: ReqRespScores,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub peer_id: String,
    pub node_id: String,
    pub user_agent: String,
    pub protocol_version: String,
    pub enr: String,
    pub addresses: Vec<String>,
    pub protocols: Vec<String>,
    pub connectedness: PeerConnectedness,
    pub direction: Direction,
    pub protected: bool,
    pub chain_id: ChainId,
    pub latency: Duration,
    pub gossip_blocks: bool,
    pub peer_scores: PeerScores,
}

impl Clone for PeerInfo {
    fn clone(&self) -> Self {
        Self {
            peer_id: self.peer_id.clone(),
            node_id: self.node_id.clone(),
            user_agent: self.user_agent.clone(),
            protocol_version: self.protocol_version.clone(),
            enr: self.enr.clone(),
            addresses: self.addresses.clone(),
            protocols: self.protocols.clone(),
            connectedness: self.connectedness.clone(),
            direction: self.direction.clone(),
            protected: self.protected,
            chain_id: self.chain_id,
            latency: self.latency,
            gossip_blocks: self.gossip_blocks,
            peer_scores: self.peer_scores.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerDump {
    pub total_connected: u32,
    pub peers: HashMap<String, PeerInfo>,
    pub banned_peers: Vec<String>,
    pub banned_ips: Vec<IpAddr>,
    // todo: should be IPNet
    pub banned_subnets: Vec<IpAddr>,
}

impl Clone for PeerDump {
    fn clone(&self) -> Self {
        Self {
            total_connected: self.total_connected,
            peers: self.peers.clone(),
            banned_peers: self.banned_peers.clone(),
            banned_ips: self.banned_ips.clone(),
            banned_subnets: self.banned_subnets.clone(),
        }
    }
}

/// Optimism specified rpc interface.
/// https://docs.optimism.io/builders/node-operators/json-rpc
/// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/node/api.go#L114
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "optimism"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "optimism"))]
pub trait OptimismApi {
    /// Get the output root at a specific block.
    #[method(name = "outputAtBlock")]
    async fn optimism_output_at_block(
        &self,
        block_number: BlockNumberOrTag,
    ) -> RpcResult<OutputResponse>;

    /// Get the synchronization status.
    #[method(name = "syncStatus")]
    async fn optimism_sync_status(&self) -> RpcResult<SyncStatus>;

    /// Get the rollup configuration parameters.
    #[method(name = "rollupConfig")]
    async fn optimism_rollup_config(&self) -> RpcResult<RollupConfig>;

    /// Get the software version.
    #[method(name = "version")]
    async fn optimism_version(&self) -> RpcResult<String>;
}

/// The opp2p namespace handles peer interactions.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "opp2p"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "opp2p"))]
pub trait OpP2PApi {
    /// Returns information of node
    #[method(name = "self")]
    async fn opp2p_self(&self) -> RpcResult<PeerInfo>;

    #[method(name = "peers")]
    async fn opp2p_peers(&self) -> RpcResult<PeerDump>;
}
