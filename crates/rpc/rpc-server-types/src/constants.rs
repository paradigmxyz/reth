use std::cmp::max;

/// The default port for the http server
pub const DEFAULT_HTTP_RPC_PORT: u16 = 8545;

/// The default port for the ws server
pub const DEFAULT_WS_RPC_PORT: u16 = 8546;

/// The default port for the auth server.
pub const DEFAULT_AUTH_PORT: u16 = 8551;

/// The default maximum block range allowed to filter
pub const DEFAULT_MAX_BLOCKS_PER_FILTER: u64 = 100_000;

/// The default maximum of logs in a single response.
pub const DEFAULT_MAX_LOGS_PER_RESPONSE: usize = 20_000;

/// The default maximum number tracing requests we're allowing concurrently.
/// Tracing is mostly CPU bound so we're limiting the number of concurrent requests to something
/// lower that the number of cores, in order to minimize the impact on the rest of the system.
pub fn default_max_tracing_requests() -> usize {
    // We reserve 2 cores for the rest of the system
    const RESERVED: usize = 2;

    std::thread::available_parallelism()
        .map_or(25, |cpus| max(cpus.get().saturating_sub(RESERVED), RESERVED))
}

/// The default IPC endpoint
#[cfg(windows)]
pub const DEFAULT_IPC_ENDPOINT: &str = r"\\.\pipe\reth.ipc";

/// The default IPC endpoint
#[cfg(not(windows))]
pub const DEFAULT_IPC_ENDPOINT: &str = "/tmp/reth.ipc";

/// The engine_api IPC endpoint
#[cfg(windows)]
pub const DEFAULT_ENGINE_API_IPC_ENDPOINT: &str = r"\\.\pipe\reth_engine_api.ipc";

/// The `engine_api` IPC endpoint
#[cfg(not(windows))]
pub const DEFAULT_ENGINE_API_IPC_ENDPOINT: &str = "/tmp/reth_engine_api.ipc";

/// GPO specific constants
pub mod gas_oracle {
    use alloy_primitives::U256;

    /// The number of transactions sampled in a block
    pub const SAMPLE_NUMBER: usize = 3_usize;

    /// The default maximum number of blocks to use for the gas price oracle.
    pub const MAX_HEADER_HISTORY: u64 = 1024;

    /// Number of recent blocks to check for gas price
    pub const DEFAULT_GAS_PRICE_BLOCKS: u32 = 20;

    /// The percentile of gas prices to use for the estimate
    pub const DEFAULT_GAS_PRICE_PERCENTILE: u32 = 60;

    /// Maximum transaction priority fee (or gas price before London Fork) to be recommended by the
    /// gas price oracle
    pub const DEFAULT_MAX_GAS_PRICE: U256 = U256::from_limbs([500_000_000_000u64, 0, 0, 0]);

    /// The default minimum gas price, under which the sample will be ignored
    pub const DEFAULT_IGNORE_GAS_PRICE: U256 = U256::from_limbs([2u64, 0, 0, 0]);
}

/// Cache specific constants
pub mod cache {
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

    /// Default number of concurrent database requests.
    pub const DEFAULT_CONCURRENT_DB_REQUESTS: usize = 512;
}
