//! Types for ethstats event reporting.
//! These structures define the data format used to report blockchain events to ethstats servers.

use alloy_consensus::Header;
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};

/// Collection of meta information about a node that is displayed on the monitoring page.
/// This information is used to identify and display node details in the ethstats monitoring
/// interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// The display name of the node in the monitoring interface
    pub name: String,

    /// The node's unique identifier
    pub node: String,

    /// The port number the node is listening on for P2P connections
    pub port: u64,

    /// The network ID the node is connected to (e.g. "1" for mainnet)
    #[serde(rename = "net")]
    pub network: String,

    /// Comma-separated list of supported protocols and their versions
    pub protocol: String,

    /// API availability indicator ("Yes" or "No")
    pub api: String,

    /// Operating system the node is running on
    pub os: String,

    /// Operating system version/architecture
    #[serde(rename = "os_v")]
    pub os_ver: String,

    /// Client software version
    pub client: String,

    /// Whether the node can provide historical block data
    #[serde(rename = "canUpdateHistory")]
    pub history: bool,
}

/// Authentication message used to login to the ethstats monitoring server.
/// Contains node identification and authentication information.
#[derive(Debug, Serialize, Deserialize)]
pub struct AuthMsg {
    /// The node's unique identifier
    pub id: String,

    /// Detailed information about the node
    pub info: NodeInfo,

    /// Secret password for authentication with the monitoring server
    pub secret: String,
}

impl AuthMsg {
    /// Generate a login message for the ethstats monitoring server.
    pub fn generate_login_message(&self) -> String {
        serde_json::json!({
            "emit": ["hello", self]
        })
        .to_string()
    }
}

/// Simplified transaction info, containing only the hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxStats {
    /// Transaction hash
    pub hash: B256,
}

/// Wrapper for uncle block headers.
/// This ensures empty lists serialize as `[]` instead of `null`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UncleStats(pub Vec<Header>);

/// Information to report about individual blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockStats {
    /// Block number (height in the chain).
    pub number: U256,

    /// Hash of this block.
    pub hash: B256,

    /// Hash of the parent block.
    #[serde(rename = "parentHash")]
    pub parent_hash: B256,

    /// Timestamp of the block (Unix time).
    pub timestamp: U256,

    /// Address of the miner who produced this block.
    pub miner: Address,

    /// Total gas used by all transactions in the block.
    #[serde(rename = "gasUsed")]
    pub gas_used: u64,

    /// Maximum gas allowed for this block.
    #[serde(rename = "gasLimit")]
    pub gas_limit: u64,

    /// Difficulty for mining this block (as a decimal string).
    #[serde(rename = "difficulty")]
    pub diff: String,

    /// Cumulative difficulty up to this block (as a decimal string).
    #[serde(rename = "totalDifficulty")]
    pub total_diff: String,

    /// Simplified list of transactions in the block.
    #[serde(rename = "transactions")]
    pub txs: Vec<TxStats>,

    /// Root hash of all transactions (Merkle root).
    #[serde(rename = "transactionsRoot")]
    pub tx_root: B256,

    /// State root after applying this block.
    #[serde(rename = "stateRoot")]
    pub root: B256,

    /// List of uncle block headers.
    pub uncles: UncleStats,
}

/// Message containing a block to be reported to the ethstats monitoring server.
#[derive(Debug, Serialize, Deserialize)]
pub struct BlockMsg {
    /// The node's unique identifier
    pub id: String,

    /// The block to report
    pub block: BlockStats,
}

impl BlockMsg {
    /// Generate a block message for the ethstats monitoring server.
    pub fn generate_block_message(&self) -> String {
        serde_json::json!({
            "emit": ["block", self]
        })
        .to_string()
    }
}

/// Message containing historical block data to be reported to the ethstats monitoring server.
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryMsg {
    /// The node's unique identifier
    pub id: String,

    /// The historical block data to report
    pub history: Vec<BlockStats>,
}

impl HistoryMsg {
    /// Generate a history message for the ethstats monitoring server.
    pub fn generate_history_message(&self) -> String {
        serde_json::json!({
            "emit": ["history", self]
        })
        .to_string()
    }
}

/// Message containing pending transaction statistics to be reported to the ethstats monitoring
/// server.
#[derive(Debug, Serialize, Deserialize)]
pub struct PendingStats {
    /// Number of pending transactions
    pub pending: u64,
}

/// Message containing pending transaction statistics to be reported to the ethstats monitoring
/// server.
#[derive(Debug, Serialize, Deserialize)]
pub struct PendingMsg {
    /// The node's unique identifier
    pub id: String,

    /// The pending transaction statistics to report
    pub stats: PendingStats,
}

impl PendingMsg {
    /// Generate a pending message for the ethstats monitoring server.
    pub fn generate_pending_message(&self) -> String {
        serde_json::json!({
            "emit": ["pending", self]
        })
        .to_string()
    }
}

/// Information reported about the local node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStats {
    /// Whether the node is active
    pub active: bool,

    /// Whether the node is currently syncing
    pub syncing: bool,

    /// Number of connected peers
    pub peers: u64,

    /// Current gas price in wei
    #[serde(rename = "gasPrice")]
    pub gas_price: u64,

    /// Node uptime percentage
    pub uptime: u64,
}

/// Message containing node statistics to be reported to the ethstats monitoring server.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatsMsg {
    /// The node's unique identifier
    pub id: String,

    /// The stats to report
    pub stats: NodeStats,
}

impl StatsMsg {
    /// Generate a stats message for the ethstats monitoring server.
    pub fn generate_stats_message(&self) -> String {
        serde_json::json!({
            "emit": ["stats", self]
        })
        .to_string()
    }
}

/// Latency report message used to report network latency to the ethstats monitoring server.
#[derive(Serialize, Deserialize, Debug)]
pub struct LatencyMsg {
    /// The node's unique identifier
    pub id: String,

    /// The latency to report in milliseconds
    pub latency: u64,
}

impl LatencyMsg {
    /// Generate a latency message for the ethstats monitoring server.
    pub fn generate_latency_message(&self) -> String {
        serde_json::json!({
            "emit": ["latency", self]
        })
        .to_string()
    }
}

/// Ping message sent to the ethstats monitoring server to initiate latency measurement.
#[derive(Serialize, Deserialize, Debug)]
pub struct PingMsg {
    /// The node's unique identifier
    pub id: String,

    /// Client timestamp when the ping was sent
    #[serde(rename = "clientTime")]
    pub client_time: String,
}

impl PingMsg {
    /// Generate a ping message for the ethstats monitoring server.
    pub fn generate_ping_message(&self) -> String {
        serde_json::json!({
            "emit": ["node-ping", self]
        })
        .to_string()
    }
}
