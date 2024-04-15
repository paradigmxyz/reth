#![allow(missing_docs)]
#![allow(unreachable_pub)]
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{Address, BlockNumber, ChainId, B256, U64};
use reth_rpc_types::BlockNumberOrTag;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr};

/// todo: move to reth_rpc_types

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockId {
    pub hash: B256,
    pub number: BlockNumber,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/id.go#L33
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L2BlockRef {
    pub hash: B256,
    pub number: BlockNumber,
    pub parent_hash: B256,
    pub timestamp: U64,
    pub l1origin: BlockId,
    pub sequence_number: u64,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/id.go#L52
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1BlockRef {
    pub hash: B256,
    pub number: BlockNumber,
    pub parent_hash: B256,
    pub timestamp: u64,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/sync_status.go#L5
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub current_l1: L1BlockRef,
    pub current_l1_finalized: L1BlockRef,
    pub head_l1: L1BlockRef,
    pub safe_l1: L1BlockRef,
    pub finalized_l1: L1BlockRef,
    pub unsafe_l2: L2BlockRef,
    pub safe_l2: L2BlockRef,
    pub finalized_l2: L2BlockRef,
    pub pending_safe_l2: L2BlockRef,
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

/// https://github.com/ethereum-optimism/optimism/blob/c7ad0ebae5dca3bf8aa6f219367a95c15a15ae41/op-service/eth/types.go#L371
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemConfig {
    pub batcher_addr: Address,
    pub overhead: B256,
    pub scalar: B256,
    pub gas_limit: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    pub l1: BlockId,
    pub l2: BlockId,
    pub l2_time: u64,
    pub system_config: SystemConfig,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/rollup/types.go#L53
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollupConfig {
    pub genesis: Genesis,
    pub block_time: u64,
    pub max_sequencer_drift: u64,
    pub seq_window_size: u64,
    pub channel_timeout: u64,
    /// todo use u128 to represent *big.Int?
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l1_chain_id: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub l2_chain_id: Option<u128>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub regolith_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canyon_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecotone_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fjord_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interop_time: Option<u64>,
    pub batch_inbox_address: Address,
    pub deposit_contract_address: Address,
    pub l1_system_config_address: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_versions_address: Option<Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub da_challenge_address: Option<Address>,
    pub da_challenge_window: u64,
    pub da_resolve_window: u64,
    pub use_plasma: bool,
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

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/p2p/rpc_api.go#L15
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    #[serde(rename = "peerID")]
    pub peer_id: String,
    #[serde(rename = "nodeID")]
    pub node_id: String,
    pub user_agent: String,
    pub protocol_version: String,
    #[serde(rename = "ENR")]
    pub enr: String,
    pub addresses: Vec<String>,
    pub protocols: Option<Vec<String>>,
    /// 0: "NotConnected", 1: "Connected",
    /// 2: "CanConnect" (gracefully disconnected)
    /// 3: "CannotConnect" (tried but failed)
    pub connectedness: u8,
    /// 0: "Unknown", 1: "Inbound" (if the peer contacted us)
    /// 2: "Outbound" (if we connected to them)
    pub direction: u8,
    pub protected: bool,
    #[serde(rename = "chainID")]
    pub chain_id: ChainId,
    pub latency: u64,
    pub gossip_blocks: bool,
    #[serde(rename = "scores")]
    pub peer_scores: PeerScores,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerDump {
    pub total_connected: u32,
    pub peers: HashMap<String, PeerInfo>,
    pub banned_peers: Vec<String>,
    #[serde(rename = "bannedIPS")]
    pub banned_ips: Vec<IpAddr>,
    // todo: should be IPNet
    pub banned_subnets: Vec<IpAddr>,
}

/// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/p2p/rpc_server.go#L203
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerStats {
    pub connected: u32,
    pub table: u32,
    #[serde(rename = "blocksTopic")]
    pub blocks_topic: u32,
    #[serde(rename = "blocksTopicV2")]
    pub blocks_topic_v2: u32,
    #[serde(rename = "blocksTopicV3")]
    pub blocks_topic_v3: u32,
    pub banned: u32,
    pub known: u32,
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

    #[method(name = "peerStats")]
    async fn opp2p_peer_stats(&self) -> RpcResult<PeerStats>;

    #[method(name = "discoveryTable")]
    async fn opp2p_discovery_table(&self) -> RpcResult<Vec<String>>;

    #[method(name = "blockPeer")]
    async fn opp2p_block_peer(&self, peer: String) -> RpcResult<()>;

    #[method(name = "listBlockedPeers")]
    async fn opp2p_list_blocked_peers(&self) -> RpcResult<Vec<String>>;

    #[method(name = "blocAddr")]
    async fn opp2p_block_addr(&self, ip: IpAddr) -> RpcResult<()>;

    #[method(name = "unblockAddr")]
    async fn opp2p_unblock_addr(&self, ip: IpAddr) -> RpcResult<()>;

    #[method(name = "listBlockedAddrs")]
    async fn opp2p_list_blocked_addrs(&self) -> RpcResult<Vec<IpAddr>>;

    /// todo: should be IPNet?
    #[method(name = "blockSubnet")]
    async fn opp2p_block_subnet(&self, subnet: String) -> RpcResult<()>;

    /// todo: should be IPNet?
    #[method(name = "unblockSubnet")]
    async fn opp2p_unblock_subnet(&self, subnet: String) -> RpcResult<()>;

    /// todo: should be IPNet?
    #[method(name = "listBlockedSubnets")]
    async fn opp2p_list_blocked_subnets(&self) -> RpcResult<Vec<String>>;

    #[method(name = "protectPeer")]
    async fn opp2p_protect_peer(&self, peer: String) -> RpcResult<()>;

    #[method(name = "unprotectPeer")]
    async fn opp2p_unprotect_peer(&self, peer: String) -> RpcResult<()>;

    #[method(name = "connectPeer")]
    async fn opp2p_connect_peer(&self, peer: String) -> RpcResult<()>;

    #[method(name = "disconnectPeer")]
    async fn opp2p_disconnect_peer(&self, peer: String) -> RpcResult<()>;
}

/// https://github.com/ethereum-optimism/optimism/blob/c7ad0ebae5dca3bf8aa6f219367a95c15a15ae41/op-node/node/api.go#L28-L36
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "admin"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "admin"))]
pub trait OpAdminApi {
    #[method(name = "resetDerivationPipeline")]
    async fn admin_reset_derivation_pipeline(&self) -> RpcResult<()>;

    #[method(name = "startSequencer")]
    async fn admin_start_sequencer(&self, block_hash: B256) -> RpcResult<()>;

    #[method(name = "stopSequencer")]
    async fn admin_stop_sequencer(&self) -> RpcResult<B256>;

    #[method(name = "sequencerActive")]
    async fn admin_sequencer_active(&self) -> RpcResult<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_helper<'a, T>(json_str: &'a str)
    where
        T: Serialize + Deserialize<'a>,
    {
        let deserialize = serde_json::from_str::<T>(json_str).unwrap();
        assert_eq!(json!(json_str), json!(serde_json::to_string_pretty(&deserialize).unwrap()));
    }

    #[test]
    fn serialize_sync_status() {
        let sync_status_json = r#"{
  "current_l1": {
    "hash": "0xff3b3253058411b727ac662f4c9ae1698918179e02ecebd304beb1a1ae8fc4fd",
    "number": 4427350,
    "parentHash": "0xb26586390c3f04678706dde13abfb5c6e6bb545e59c22774e651db224b16cd48",
    "timestamp": 1696478784
  },
  "current_l1_finalized": {
    "hash": "0x7157f91b8ae21ef869c604e5b268e392de5aa69a9f44466b9b0f838d56426541",
    "number": 4706784,
    "parentHash": "0x1ac2612a500b9facd650950b8755d97cf2470818da2d88552dea7cd563e86a17",
    "timestamp": 1700160084
  },
  "head_l1": {
    "hash": "0x6110a8e6ed4c4aaab20477a3eac81bf99e505bf6370cd4d2e3c6d34aa5f4059a",
    "number": 4706863,
    "parentHash": "0xee8a9cba5d93481f11145c24890fd8f536384f3c3c043f40006650538fbdcb56",
    "timestamp": 1700161272
  },
  "safe_l1": {
    "hash": "0x8407c9968ce278ab435eeaced18ba8f2f94670ad9d3bdd170560932cf46e2804",
    "number": 4706811,
    "parentHash": "0x6593cccab3e772776418ff691f6e4e75597af18505373522480fdd97219c06ef",
    "timestamp": 1700160480
  },
  "finalized_l1": {
    "hash": "0x7157f91b8ae21ef869c604e5b268e392de5aa69a9f44466b9b0f838d56426541",
    "number": 4706784,
    "parentHash": "0x1ac2612a500b9facd650950b8755d97cf2470818da2d88552dea7cd563e86a17",
    "timestamp": 1700160084
  },
  "unsafe_l2": {
    "hash": "0x9a3b2edab72150de252d45cabe2f1ac57d48ddd52bb891831ffed00e89408fe4",
    "number": 2338094,
    "parentHash": "0x935b94ec0bac0e63c67a870b1a97d79e3fa84dda86d31996516cb2f940753f53",
    "timestamp": 1696478728,
    "l1origin": {
      "hash": "0x38731e0a6eeb40091f0c4a00650e911c57d054aaeb5b158f55cd5705fa6a3ebf",
      "number": 4427339
    },
    "sequenceNumber": 3
  },
  "safe_l2": {
    "hash": "0x9a3b2edab72150de252d45cabe2f1ac57d48ddd52bb891831ffed00e89408fe4",
    "number": 2338094,
    "parentHash": "0x935b94ec0bac0e63c67a870b1a97d79e3fa84dda86d31996516cb2f940753f53",
    "timestamp": 1696478728,
    "l1origin": {
      "hash": "0x38731e0a6eeb40091f0c4a00650e911c57d054aaeb5b158f55cd5705fa6a3ebf",
      "number": 4427339
    },
    "sequenceNumber": 3
  },
  "finalized_l2": {
    "hash": "0x285b03afb46faad747be1ca7ab6ef50ef0ff1fe04e4eeabafc54f129d180fad2",
    "number": 2337942,
    "parentHash": "0x7e7f36cba1fd1ccdcdaa81577a1732776a01c0108ab5f98986cf997724eb48ac",
    "timestamp": 1696478424,
    "l1origin": {
      "hash": "0x983309dadf7e0ab8447f3050f2a85b179e9acde1cd884f883fb331908c356412",
      "number": 4427314
    },
    "sequenceNumber": 7
  },
  "pending_safe_l2": {
    "hash": "0x9a3b2edab72150de252d45cabe2f1ac57d48ddd52bb891831ffed00e89408fe4",
    "number": 2338094,
    "parentHash": "0x935b94ec0bac0e63c67a870b1a97d79e3fa84dda86d31996516cb2f940753f53",
    "timestamp": 1696478728,
    "l1origin": {
      "hash": "0x38731e0a6eeb40091f0c4a00650e911c57d054aaeb5b158f55cd5705fa6a3ebf",
      "number": 4427339
    },
    "sequenceNumber": 3
  },
  "queued_unsafe_l2": {
    "hash": "0x3af253f5b993f58fffdd5e594b3f53f5b7b254cdc18f4bdb13ea7331149942db",
    "number": 4054795,
    "parentHash": "0x284b7dc92bac97be8ec3b2cf548e75208eb288704de381f2557938ecdf86539d",
    "timestamp": 1699912130,
    "l1origin": {
      "hash": "0x1490a63c372090a0331e05e63ec6a7a6e84835f91776306531f28b4217394d76",
      "number": 4688196
    },
    "sequenceNumber": 2
  },
  "engine_sync_target": {
    "hash": "0x9a3b2edab72150de252d45cabe2f1ac57d48ddd52bb891831ffed00e89408fe4",
    "number": 2338094,
    "parentHash": "0x935b94ec0bac0e63c67a870b1a97d79e3fa84dda86d31996516cb2f940753f53",
    "timestamp": 1696478728,
    "l1origin": {
      "hash": "0x38731e0a6eeb40091f0c4a00650e911c57d054aaeb5b158f55cd5705fa6a3ebf",
      "number": 4427339
    },
    "sequenceNumber": 3
  }
}"#;
        test_helper::<SyncStatus>(sync_status_json);
    }

    #[test]
    fn test_rollup_config() {
        let rollup_config_json = r#"{
  "genesis": {
    "l1": {
      "hash": "0x48f520cf4ddaf34c8336e6e490632ea3cf1e5e93b0b2bc6e917557e31845371b",
      "number": 4071408
    },
    "l2": {
      "hash": "0x102de6ffb001480cc9b8b548fd05c34cd4f46ae4aa91759393db90ea0409887d",
      "number": 0
    },
    "l2_time": 1691802540,
    "system_config": {
      "batcherAddr": "0x8f23bb38f531600e5d8fddaaec41f13fab46e98c",
      "overhead": "0x00000000000000000000000000000000000000000000000000000000000000bc",
      "scalar": "0x00000000000000000000000000000000000000000000000000000000000a6fe0",
      "gasLimit": 30000000
    }
  },
  "block_time": 2,
  "max_sequencer_drift": 600,
  "seq_window_size": 3600,
  "channel_timeout": 300,
  "l1_chain_id": 11155111,
  "l2_chain_id": 11155420,
  "regolith_time": 0,
  "canyon_time": 1699981200,
  "batch_inbox_address": "0xff00000000000000000000000000000011155420",
  "deposit_contract_address": "0x16fc5058f25648194471939df75cf27a2fdc48bc",
  "l1_system_config_address": "0x034edd2a225f7f429a63e0f1d2084b9e0a93b538",
  "protocol_versions_address": "0x79add5713b383daa0a138d3c4780c7a1804a8090",
  "da_challenge_window": 12,
  "da_resolve_window": 12,
  "use_plasma": true
}"#;
        test_helper::<RollupConfig>(rollup_config_json);
    }

    #[test]
    fn test_peer_info() {
        let peer_info_json = r#"{
  "peerID": "16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE",
  "nodeID": "75a52a90fe5f972171fefce2399ca5a73191c654e7c7ddfdd71edf4fca6697f0",
  "userAgent": "",
  "protocolVersion": "",
  "ENR": "enr:-J-4QFOtI_hDBa_kilrQcg4iTJt9VMAuDLCbgAAKMa--WfxoPml1xDYxypUG7IsWga83FOlvr78LG3oH8CfzRzUmsDyGAYvKqIZ2gmlkgnY0gmlwhGxAaceHb3BzdGFja4Xc76gFAIlzZWNwMjU2azGhAnAON-FvpiWY2iG_LXJDYosknGyikaajPDd1cQARsVnBg3RjcIIkBoN1ZHCC0Vs",
  "addresses": [
    "/ip4/127.0.0.1/tcp/9222/p2p/16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE",
    "/ip4/192.168.1.71/tcp/9222/p2p/16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE",
    "/ip4/108.64.105.199/tcp/9222/p2p/16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE"
  ],
  "protocols": null,
  "connectedness": 0,
  "direction": 0,
  "protected": false,
  "chainID": 0,
  "latency": 0,
  "gossipBlocks": true,
  "scores": {
    "gossip": {
      "total": 0.0,
      "blocks": {
        "timeInMesh": 0.0,
        "firstMessageDeliveries": 0.0,
        "meshMessageDeliveries": 0.0,
        "invalidMessageDeliveries": 0.0
      },
      "IPColocationFactor": 0.0,
      "behavioralPenalty": 0.0
    },
    "reqResp": {
      "validResponses": 0.0,
      "errorResponses": 0.0,
      "rejectedPayloads": 0.0
    }
  }
}"#;
        test_helper::<PeerInfo>(peer_info_json);
    }

    #[test]
    fn test_peer_dump() {
        let peer_dump_json = r#"{
  "totalConnected": 20,
  "peers": {
    "16Uiu2HAkvNYscHu4V1uj6fVWkwrAMCRsqXDSq4mUbhpGq4LttYsC": {
      "peerID": "16Uiu2HAkvNYscHu4V1uj6fVWkwrAMCRsqXDSq4mUbhpGq4LttYsC",
      "nodeID": "d693c5b58424016c0c38ec5539c272c754cb6b8007b322e0ecf16a4ee13f96fb",
      "userAgent": "optimism",
      "protocolVersion": "",
      "ENR": "",
      "addresses": [
        "/ip4/20.249.62.215/tcp/9222/p2p/16Uiu2HAkvNYscHu4V1uj6fVWkwrAMCRsqXDSq4mUbhpGq4LttYsC"
      ],
      "protocols": [
        "/ipfs/ping/1.0.0",
        "/meshsub/1.0.0",
        "/meshsub/1.1.0",
        "/opstack/req/payload_by_number/11155420/0",
        "/floodsub/1.0.0",
        "/ipfs/id/1.0.0",
        "/ipfs/id/push/1.0.0"
      ],
      "connectedness": 1,
      "direction": 1,
      "protected": false,
      "chainID": 0,
      "latency": 0,
      "gossipBlocks": true,
      "scores": {
        "gossip": {
          "total": -5.04,
          "blocks": {
            "timeInMesh": 0.0,
            "firstMessageDeliveries": 0.0,
            "meshMessageDeliveries": 0.0,
            "invalidMessageDeliveries": 0.0
          },
          "IPColocationFactor": 0.0,
          "behavioralPenalty": 0.0
        },
        "reqResp": {
          "validResponses": 0.0,
          "errorResponses": 0.0,
          "rejectedPayloads": 0.0
        }
      }
    }
  },
  "bannedPeers": [],
  "bannedIPS": [],
  "bannedSubnets": []
}"#;
        test_helper::<PeerDump>(peer_dump_json);
    }

    #[test]
    fn test_peer_stat() {
        let peer_stat_json = r#"{
  "connected": 20,
  "table": 94,
  "blocksTopic": 20,
  "blocksTopicV2": 18,
  "blocksTopicV3": 20,
  "banned": 0,
  "known": 71
}"#;
        test_helper::<PeerStats>(peer_stat_json);
    }
}
