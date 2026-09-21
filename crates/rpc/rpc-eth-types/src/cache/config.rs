//! Configuration for RPC cache.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use reth_rpc_server_types::constants::cache::{
    DEFAULT_BAL_CACHE_MAX_LEN, DEFAULT_BLOCK_CACHE_MAX_LEN, DEFAULT_CONCURRENT_DB_REQUESTS,
    DEFAULT_MAX_CACHED_TX_HASHES, DEFAULT_RECEIPT_CACHE_MAX_LEN,
};

/// Settings for the [`EthStateCache`](super::EthStateCache).
///
/// Byte budgets cover estimated payload sizes, excluding map and request queue overhead. Shared
/// payloads can remain allocated after eviction while other components still hold references.
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
    /// Max number of EVM BALs in cache.
    ///
    /// Default is 1000.
    pub max_bals: u32,
    /// Maximum estimated memory retained by the block cache, in bytes.
    ///
    /// `None` disables the byte limit; `Some(0)` disables caching. The entry count limit also
    /// applies.
    #[serde(default)]
    pub max_blocks_bytes: Option<usize>,
    /// Maximum estimated memory retained by the receipts cache, in bytes.
    ///
    /// `None` disables the byte limit; `Some(0)` disables caching. The entry count limit also
    /// applies.
    #[serde(default)]
    pub max_receipts_bytes: Option<usize>,
    /// Maximum estimated memory retained by the BAL cache, in bytes.
    ///
    /// `None` disables the byte limit; `Some(0)` disables caching. The entry count limit also
    /// applies.
    #[serde(default)]
    pub max_bals_bytes: Option<usize>,
    /// Duration after which an unused block, receipts collection, or BAL is evicted.
    ///
    /// Successful lookups refresh the timeout. `None` or a zero duration disables expiration.
    #[serde(default)]
    pub idle_timeout: Option<Duration>,
    /// Cache BALs computed by RPC requests for transaction tracing. Disabled by default.
    #[serde(default)]
    pub cache_computed_bals: bool,
    /// Prewarm BALs until native BAL support, optionally replaying this many recent blocks on
    /// startup. `Some(0)` prewarms only new canonical blocks; `None` disables prewarming.
    ///
    /// Implies caching of BALs computed by RPC requests.
    #[serde(default)]
    pub prewarm_bals: Option<usize>,
    /// Max number of concurrent database requests.
    ///
    /// Default is 512.
    pub max_concurrent_db_requests: usize,
    /// Maximum number of transaction hashes to cache for transaction lookups.
    pub max_cached_tx_hashes: u32,
}

impl Default for EthStateCacheConfig {
    fn default() -> Self {
        Self {
            max_blocks: DEFAULT_BLOCK_CACHE_MAX_LEN,
            max_receipts: DEFAULT_RECEIPT_CACHE_MAX_LEN,
            max_bals: DEFAULT_BAL_CACHE_MAX_LEN,
            max_blocks_bytes: None,
            max_receipts_bytes: None,
            max_bals_bytes: None,
            idle_timeout: None,
            cache_computed_bals: false,
            prewarm_bals: None,
            max_concurrent_db_requests: DEFAULT_CONCURRENT_DB_REQUESTS,
            max_cached_tx_hashes: DEFAULT_MAX_CACHED_TX_HASHES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_cache_config_keeps_count_only_limits() {
        let config: EthStateCacheConfig = serde_json::from_value(serde_json::json!({
            "maxBlocks": 5000,
            "maxReceipts": 2000,
            "maxBals": 1000,
            "maxConcurrentDbRequests": 512,
            "maxCachedTxHashes": 100000
        }))
        .unwrap();

        assert_eq!(config, EthStateCacheConfig::default());
        assert_eq!(config.max_blocks_bytes, None);
        assert_eq!(config.max_receipts_bytes, None);
        assert_eq!(config.max_bals_bytes, None);
        assert_eq!(config.idle_timeout, None);
    }

    #[test]
    fn cache_memory_limits_and_idle_timeout_round_trip() {
        for config in [
            EthStateCacheConfig::default(),
            EthStateCacheConfig {
                max_blocks_bytes: Some(1024),
                max_receipts_bytes: Some(2048),
                max_bals_bytes: Some(4096),
                idle_timeout: Some(Duration::from_millis(1500)),
                ..Default::default()
            },
            EthStateCacheConfig {
                max_blocks_bytes: Some(0),
                max_receipts_bytes: Some(0),
                max_bals_bytes: Some(0),
                idle_timeout: Some(Duration::ZERO),
                ..Default::default()
            },
        ] {
            let serialized = serde_json::to_value(config).unwrap();
            assert_eq!(serialized["maxBlocksBytes"], serde_json::json!(config.max_blocks_bytes));
            assert_eq!(
                serialized["maxReceiptsBytes"],
                serde_json::json!(config.max_receipts_bytes)
            );
            assert_eq!(serialized["maxBalsBytes"], serde_json::json!(config.max_bals_bytes));
            assert_eq!(serde_json::from_value::<EthStateCacheConfig>(serialized).unwrap(), config);
        }
    }
}
