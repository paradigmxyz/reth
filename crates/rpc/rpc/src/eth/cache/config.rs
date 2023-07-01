use serde::{Deserialize, Serialize};

// TODO: memory based limiter is currently disabled pending <https://github.com/paradigmxyz/reth/issues/3503>
/// Default cache size for the block cache: 500MB
///
/// With an average block size of ~100kb this should be able to cache ~5000 blocks.
pub const DEFAULT_BLOCK_CACHE_SIZE_BYTES_MB: usize = 500;

/// Default cache size for the receipts cache: 500MB
pub const DEFAULT_RECEIPT_CACHE_SIZE_BYTES_MB: usize = 500;

/// Default cache size for the env cache: 1MB
pub const DEFAULT_ENV_CACHE_SIZE_BYTES_MB: usize = 1;

/// Default cache size for the block cache: 5000 blocks.
pub const DEFAULT_BLOCK_CACHE_MAX_LEN: u32 = 5000;

/// Default cache size for the receipts cache: 2000 receipts.
pub const DEFAULT_RECEIPT_CACHE_MAX_LEN: u32 = 2000;

/// Default cache size for the env cache: 1000 envs.
pub const DEFAULT_ENV_CACHE_MAX_LEN: u32 = 1000;

/// Settings for the [EthStateCache](crate::eth::cache::EthStateCache).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
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
    /// Max number of bytes for cached env data.
    ///
    /// Default is 1000.
    pub max_envs: u32,
}

impl Default for EthStateCacheConfig {
    fn default() -> Self {
        Self {
            max_blocks: DEFAULT_BLOCK_CACHE_MAX_LEN,
            max_receipts: DEFAULT_RECEIPT_CACHE_MAX_LEN,
            max_envs: DEFAULT_ENV_CACHE_MAX_LEN,
        }
    }
}
