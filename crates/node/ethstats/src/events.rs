//! Binding all events related to blockStats

use alloy_consensus::Header;
use alloy_primitives::{Address, B256, U256};
// use serde::{
//     ser::{Serialize, Serializer},
//     Deserialize,
// };

/// BlockStats is the information to report about individual blocks.
#[derive(Debug, Clone)]
pub struct BlockStats {
    /// Block number (height) in the blockchain
    pub number: Option<U256>,
    /// Hash of the block
    pub hash: B256,
    /// Hash of the parent block
    pub parent_hash: B256,
    /// Number of seconds since the Unix epoch.
    pub timestamp: Option<U256>,
    /// Address of the miner who mined the block
    pub miner: Address,
    /// Total gas used by all transactions in the block
    pub gas_used: u64,
    /// Maximum amount of gas allowed in the block
    pub gas_limit: u64,
    /// Block difficulty
    pub difficulty: String,
    /// Cumulative difficulty of the entire chain up to this block.
    pub total_difficulty: String,
    /// List of transaction stats in this block.
    pub transactions: Vec<TxStats>,
    /// Merkle root hash of all transactions in the block
    pub transactions_root: B256,
    /// Merkle root hash of the world state
    pub state_root: B256,
    /// List of uncle (ommer) block headers included in this block
    pub uncles: UncleStats,
}

/// TxStats is the information to report about individual transactions.
#[derive(Debug, Clone)]
pub struct TxStats {
    /// Hash of the transaction
    pub hash: B256,
}

/// uncleStats is a custom wrapper around an uncle array to force serializing
/// empty arrays instead of returning null for them.
#[derive(Debug, Clone)]
pub struct UncleStats(pub Vec<Header>);
