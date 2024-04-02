#![allow(missing_docs)]
#![allow(unreachable_pub)]
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{Address, BlockNumber, ChainId, B256};
use reth_rpc_types::{BlockId, BlockNumberOrTag};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::IpAddr};

// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/id.go#L33
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L2BlockRef {
    pub hash: B256,
    pub number: BlockNumber,
    pub parent_hash: B256,
    pub timestamp: u64,
    pub l1origin: BlockId,
    pub sequence_number: u64,
}

// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/id.go#L52
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1BlockRef {
    pub hash: B256,
    pub number: BlockNumber,
    pub parent_hash: B256,
    pub timestamp: u64,
}

// https://github.com/ethereum-optimism/optimism/blob/develop/op-service/eth/sync_status.go#L5
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

// https://github.com/ethereum-optimism/optimism/blob/c7ad0ebae5dca3bf8aa6f219367a95c15a15ae41/op-service/eth/types.go#L371
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

// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/rollup/types.go#L53
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollupConfig {
    pub genesis: Genesis,
    pub block_time: u64,
    pub max_sequencer_drift: u64,
    pub seq_window_size: u64,
    pub channel_timeout: u64,
    /// todo use u128 to represent *big.Int?
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l1_chain_id: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l2_chain_id: Option<u128>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regolith_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canyon_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecotone_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fjord_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interop_time: Option<u64>,
    pub batch_inbox_address: Address,
    pub deposit_contract_address: Address,
    pub l1_system_config_address: Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_versions_address: Option<Address>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub da_challenge_address: Option<Address>,
    pub da_challenge_window: u64,
    pub da_resolve_window: u64,
    pub use_plasma: bool,
}

// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L13
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicScores {
    pub time_in_mesh: f64,
    pub first_message_deliveries: f64,
    pub mesh_message_deliveries: f64,
    pub invalid_message_deliveries: f64,
}

// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L20C6-L20C18
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GossipScores {
    pub total: f64,
    pub blocks: TopicScores,
    #[serde(rename = "IPColocationFactor")]
    pub ip_colocation_factor: f64,
    pub behavioral_penalty: f64,
}

// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L31C1-L35C2
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReqRespScores {
    pub valid_responses: f64,
    pub error_responses: f64,
    pub rejected_payloads: f64,
}

// https://github.com/ethereum-optimism/optimism/blob/8dd17a7b114a7c25505cd2e15ce4e3d0f7e3f7c1/op-node/p2p/store/iface.go#L81
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerScores {
    pub gossip: GossipScores,
    pub req_resp: ReqRespScores,
}

// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/p2p/rpc_api.go#L15
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
    /// nanosecond
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

// https://github.com/ethereum-optimism/optimism/blob/develop/op-node/p2p/rpc_server.go#L203
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

/// The admin namespace endpoints
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
        assert_eq!(json!(json_str), json!(serde_json::to_string(&deserialize).unwrap()));
    }

    #[test]
    fn test_output_response() {
        let output_response_json = r#"{"version":"0x0000000000000000000000000000000000000000000000000000000000000000","outputRoot":"0xf1119e7d0fef8c54ab799be80fc61f503cea4e5c0aa1cf7ac104ef3a104f3bd1","blockRef":{"hash":"0x6d39c46aabc847f5f2664a22bbc5f65a57286603095a9ebc946d1ed19ef4925c","number":118818299,"parentHash":"0x8a0876a165da864c223d30e444b1c003fb59920c88dfb12157c0f83826e0f8ed","timestamp":1713235375,"l1origin":{"blockHash":"0x807da416f5aaa26fa228e0cf53e76fab783b56d7996c717663335b40e0b28824"},"sequenceNumber":4},"withdrawalStorageRoot":"0x5c9a29a8ad2ecf97fb4bdea74c715fd2c13fa87d4861414478bc4579601c3585","stateRoot":"0x16849c0a93d00bb2d7ceacda11a1478854d2bbb0a377b4d6793b67a3f05eb6fe","syncStatus":{"current_l1":{"hash":"0x2f0f186d0fece338aa563f5dfc49a73cba5607445ff87aca833fd1d6833c5e05","number":19661406,"parentHash":"0x2c7c564d2960c8035fa6962ebf071668fdcdf8ca004bca5adfd04166ce32aacc","timestamp":1713190115},"current_l1_finalized":{"hash":"0xbd916c8552f5dcd68d2cc836a4d173426e85e6625845cfb3fb60610d383670db","number":19665084,"parentHash":"0xe16fade2cddae87d0f9487600481f980619a138de735c97626239edf08c53275","timestamp":1713234647},"head_l1":{"hash":"0xf98493dcc3d82fe9af339c0a81b0f96172a56764f9abcff464c740e0cb3ccee7","number":19665175,"parentHash":"0xfbab86e5b807916c7ddfa395db794cdf4162128b9770eb8eb829679d81d74328","timestamp":1713235763},"safe_l1":{"hash":"0xfb8f07e551eb65c3282aaefe9a4954c15672e0077b2a5a1db18fcd2126cbc922","number":19665115,"parentHash":"0xfc0d62788fb9cda1cacb54a0e53ca398289436a6b68d1ba69db2942500b4ce5f","timestamp":1713235031},"finalized_l1":{"hash":"0xbd916c8552f5dcd68d2cc836a4d173426e85e6625845cfb3fb60610d383670db","number":19665084,"parentHash":"0xe16fade2cddae87d0f9487600481f980619a138de735c97626239edf08c53275","timestamp":1713234647},"unsafe_l2":{"hash":"0x3540517a260316758a4872f7626e8b9e009968b6d8cfa9c11bfd3a03e7656bd5","number":118818499,"parentHash":"0x09f30550e6d6f217691e185bf1a2b4665b83f43fc8dbcc68c0bfd513e6805590","timestamp":1713235775,"l1origin":{"blockHash":"0x036003c1c6561123a2f6573b7a34e9598bd023199e259d91765ee2c8677d9c07"},"sequenceNumber":0},"safe_l2":{"hash":"0x2e8c339104e3ce0a81c636a10ea9181acbfd3c195d43f2f2dacce8f869b1cca8","number":118795493,"parentHash":"0xaac10ffe0a2cbd572a0ee8aa0b09341ad7bbec491f0bf328dd526637617b1b4a","timestamp":1713189763,"l1origin":{"blockHash":"0x55c6ed6a81829e9dffc9c968724af657fcf8e0b497188d05476e94801eb483ce"},"sequenceNumber":1},"finalized_l2":{"hash":"0x2e8c339104e3ce0a81c636a10ea9181acbfd3c195d43f2f2dacce8f869b1cca8","number":118795493,"parentHash":"0xaac10ffe0a2cbd572a0ee8aa0b09341ad7bbec491f0bf328dd526637617b1b4a","timestamp":1713189763,"l1origin":{"blockHash":"0x55c6ed6a81829e9dffc9c968724af657fcf8e0b497188d05476e94801eb483ce"},"sequenceNumber":1},"pending_safe_l2":{"hash":"0x2e8c339104e3ce0a81c636a10ea9181acbfd3c195d43f2f2dacce8f869b1cca8","number":118795493,"parentHash":"0xaac10ffe0a2cbd572a0ee8aa0b09341ad7bbec491f0bf328dd526637617b1b4a","timestamp":1713189763,"l1origin":{"blockHash":"0x55c6ed6a81829e9dffc9c968724af657fcf8e0b497188d05476e94801eb483ce"},"sequenceNumber":1}}}"#;
        test_helper::<OutputResponse>(output_response_json);
    }

    #[test]
    fn serialize_sync_status() {
        let sync_status_json = r#"{"current_l1":{"hash":"0x2f0f186d0fece338aa563f5dfc49a73cba5607445ff87aca833fd1d6833c5e05","number":19661406,"parentHash":"0x2c7c564d2960c8035fa6962ebf071668fdcdf8ca004bca5adfd04166ce32aacc","timestamp":1713190115},"current_l1_finalized":{"hash":"0x4d769506bbfe27051715225af5ec4189f6bbd235b6d32db809dd8f5a03737b03","number":19665052,"parentHash":"0xc6324687f2baf8cc48eebd15df3a461b2b2838b5f5b16615531fc31788edb8c4","timestamp":1713234263},"head_l1":{"hash":"0xfc5ab77c6c08662a3b4d85b8c86010b7aecfc2c0369e4458f80357530db8e919","number":19665141,"parentHash":"0x099792a293002b987f3507524b28614f399b2b5ed607788520963c251844113c","timestamp":1713235355},"safe_l1":{"hash":"0xbd916c8552f5dcd68d2cc836a4d173426e85e6625845cfb3fb60610d383670db","number":19665084,"parentHash":"0xe16fade2cddae87d0f9487600481f980619a138de735c97626239edf08c53275","timestamp":1713234647},"finalized_l1":{"hash":"0x4d769506bbfe27051715225af5ec4189f6bbd235b6d32db809dd8f5a03737b03","number":19665052,"parentHash":"0xc6324687f2baf8cc48eebd15df3a461b2b2838b5f5b16615531fc31788edb8c4","timestamp":1713234263},"unsafe_l2":{"hash":"0x6d39c46aabc847f5f2664a22bbc5f65a57286603095a9ebc946d1ed19ef4925c","number":118818299,"parentHash":"0x8a0876a165da864c223d30e444b1c003fb59920c88dfb12157c0f83826e0f8ed","timestamp":1713235375,"l1origin":{"blockHash":"0x807da416f5aaa26fa228e0cf53e76fab783b56d7996c717663335b40e0b28824"},"sequenceNumber":4},"safe_l2":{"hash":"0x2e8c339104e3ce0a81c636a10ea9181acbfd3c195d43f2f2dacce8f869b1cca8","number":118795493,"parentHash":"0xaac10ffe0a2cbd572a0ee8aa0b09341ad7bbec491f0bf328dd526637617b1b4a","timestamp":1713189763,"l1origin":{"blockHash":"0x55c6ed6a81829e9dffc9c968724af657fcf8e0b497188d05476e94801eb483ce"},"sequenceNumber":1},"finalized_l2":{"hash":"0x2e8c339104e3ce0a81c636a10ea9181acbfd3c195d43f2f2dacce8f869b1cca8","number":118795493,"parentHash":"0xaac10ffe0a2cbd572a0ee8aa0b09341ad7bbec491f0bf328dd526637617b1b4a","timestamp":1713189763,"l1origin":{"blockHash":"0x55c6ed6a81829e9dffc9c968724af657fcf8e0b497188d05476e94801eb483ce"},"sequenceNumber":1},"pending_safe_l2":{"hash":"0x2e8c339104e3ce0a81c636a10ea9181acbfd3c195d43f2f2dacce8f869b1cca8","number":118795493,"parentHash":"0xaac10ffe0a2cbd572a0ee8aa0b09341ad7bbec491f0bf328dd526637617b1b4a","timestamp":1713189763,"l1origin":{"blockHash":"0x55c6ed6a81829e9dffc9c968724af657fcf8e0b497188d05476e94801eb483ce"},"sequenceNumber":1}}"#;
        test_helper::<SyncStatus>(sync_status_json);
    }

    #[test]
    fn test_rollup_config() {
        let rollup_config_json = r#"{"genesis":{"l1":{"blockHash":"0x438335a20d98863a4c0c97999eb2481921ccd28553eac6f913af7c12aec04108"},"l2":{"blockHash":"0xdbf6a80fef073de06add9b0d14026d6e5a86c85f6d102c36d3d8e9cf89c2afd3"},"l2_time":1686068903,"system_config":{"batcherAddr":"0x6887246668a3b87f54deb3b94ba47a6f63f32985","overhead":"0x00000000000000000000000000000000000000000000000000000000000000bc","scalar":"0x00000000000000000000000000000000000000000000000000000000000a6fe0","gasLimit":30000000}},"block_time":2,"max_sequencer_drift":600,"seq_window_size":3600,"channel_timeout":300,"l1_chain_id":1,"l2_chain_id":10,"regolith_time":0,"canyon_time":1704992401,"delta_time":1708560000,"ecotone_time":1710374401,"batch_inbox_address":"0xff00000000000000000000000000000000000010","deposit_contract_address":"0xbeb5fc579115071764c7423a4f12edde41f106ed","l1_system_config_address":"0x229047fed2591dbec1ef1118d64f7af3db9eb290","protocol_versions_address":"0x8062abc286f5e7d9428a0ccb9abd71e50d93b935","da_challenge_address":"0x0000000000000000000000000000000000000000","da_challenge_window":0,"da_resolve_window":0,"use_plasma":false}"#;
        test_helper::<RollupConfig>(rollup_config_json);
    }

    #[test]
    fn test_peer_info() {
        let peer_info_json = r#"{"peerID":"16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE","nodeID":"75a52a90fe5f972171fefce2399ca5a73191c654e7c7ddfdd71edf4fca6697f0","userAgent":"","protocolVersion":"","ENR":"enr:-J-4QFOtI_hDBa_kilrQcg4iTJt9VMAuDLCbgAAKMa--WfxoPml1xDYxypUG7IsWga83FOlvr78LG3oH8CfzRzUmsDyGAYvKqIZ2gmlkgnY0gmlwhGxAaceHb3BzdGFja4Xc76gFAIlzZWNwMjU2azGhAnAON-FvpiWY2iG_LXJDYosknGyikaajPDd1cQARsVnBg3RjcIIkBoN1ZHCC0Vs","addresses":["/ip4/127.0.0.1/tcp/9222/p2p/16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE","/ip4/192.168.1.71/tcp/9222/p2p/16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE","/ip4/108.64.105.199/tcp/9222/p2p/16Uiu2HAm2y6DXp6THWHCyquczNUh8gVAm4spo6hjP3Ns1dGRiAdE"],"protocols":null,"connectedness":0,"direction":0,"protected":false,"chainID":0,"latency":0,"gossipBlocks":true,"scores":{"gossip":{"total":0.0,"blocks":{"timeInMesh":0.0,"firstMessageDeliveries":0.0,"meshMessageDeliveries":0.0,"invalidMessageDeliveries":0.0},"IPColocationFactor":0.0,"behavioralPenalty":0.0},"reqResp":{"validResponses":0.0,"errorResponses":0.0,"rejectedPayloads":0.0}}}"#;
        test_helper::<PeerInfo>(peer_info_json);
    }

    #[test]
    fn test_peer_dump() {
        let peer_dump_json = r#"{"totalConnected":20,"peers":{"16Uiu2HAkvNYscHu4V1uj6fVWkwrAMCRsqXDSq4mUbhpGq4LttYsC":{"peerID":"16Uiu2HAkvNYscHu4V1uj6fVWkwrAMCRsqXDSq4mUbhpGq4LttYsC","nodeID":"d693c5b58424016c0c38ec5539c272c754cb6b8007b322e0ecf16a4ee13f96fb","userAgent":"optimism","protocolVersion":"","ENR":"","addresses":["/ip4/20.249.62.215/tcp/9222/p2p/16Uiu2HAkvNYscHu4V1uj6fVWkwrAMCRsqXDSq4mUbhpGq4LttYsC"],"protocols":["/ipfs/ping/1.0.0","/meshsub/1.0.0","/meshsub/1.1.0","/opstack/req/payload_by_number/11155420/0","/floodsub/1.0.0","/ipfs/id/1.0.0","/ipfs/id/push/1.0.0"],"connectedness":1,"direction":1,"protected":false,"chainID":0,"latency":0,"gossipBlocks":true,"scores":{"gossip":{"total":-5.04,"blocks":{"timeInMesh":0.0,"firstMessageDeliveries":0.0,"meshMessageDeliveries":0.0,"invalidMessageDeliveries":0.0},"IPColocationFactor":0.0,"behavioralPenalty":0.0},"reqResp":{"validResponses":0.0,"errorResponses":0.0,"rejectedPayloads":0.0}}}},"bannedPeers":[],"bannedIPS":[],"bannedSubnets":[]}"#;
        test_helper::<PeerDump>(peer_dump_json);
    }

    #[test]
    fn test_peer_stats() {
        let peer_stats_json = r#"{"connected":20,"table":94,"blocksTopic":20,"blocksTopicV2":18,"blocksTopicV3":20,"banned":0,"known":71}"#;
        test_helper::<PeerStats>(peer_stats_json);
    }
}
