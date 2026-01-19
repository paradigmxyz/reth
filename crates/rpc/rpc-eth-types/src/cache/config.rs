//! Configuration for RPC cache.

use serde::{Deserialize, Serialize};

use reth_rpc_server_types::constants::cache::{
    DEFAULT_BLOCK_CACHE_MAX_LEN, DEFAULT_CONCURRENT_DB_REQUESTS, DEFAULT_HEADER_CACHE_MAX_LEN,
    DEFAULT_RECEIPT_CACHE_MAX_LEN,
};

/// Default number of transactions per block for sizing the transaction lookup cache.
pub const DEFAULT_MAX_TXS_PER_BLOCK: u32 = 200;

/// Settings for the [`EthStateCache`](super::EthStateCache).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthStateCacheConfig {
    /// Max number of blocks in cache.
    ///
    /// Default is 5000.
    pub max_blocks: u32,
    /// Max number receipts in cache.
    ///
    /// Default is 2000.
    pub max_receipts: u32,
    /// Max number of headers in cache.
    ///
    /// Default is 1000.
    pub max_headers: u32,
    /// Max number of concurrent database requests.
    ///
    /// Default is 512.
    pub max_concurrent_db_requests: usize,
    /// Assumed number of transactions per block for sizing the transaction hash lookup cache.
    ///
    /// The total cache size will be `max_blocks * max_txs_per_block`.
    ///
    /// Default is 100.
    pub max_txs_per_block: u32,
}

impl Default for EthStateCacheConfig {
    fn default() -> Self {
        Self {
            max_blocks: DEFAULT_BLOCK_CACHE_MAX_LEN,
            max_receipts: DEFAULT_RECEIPT_CACHE_MAX_LEN,
            max_headers: DEFAULT_HEADER_CACHE_MAX_LEN,
            max_concurrent_db_requests: DEFAULT_CONCURRENT_DB_REQUESTS,
            max_txs_per_block: DEFAULT_MAX_TXS_PER_BLOCK,
        }
    }
}
