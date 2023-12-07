use clap::{builder::RangedU64ValueParser, Args};
use reth_rpc::eth::cache::{
    DEFAULT_BLOCK_CACHE_MAX_LEN, DEFAULT_CONCURRENT_DB_REQUESTS, DEFAULT_ENV_CACHE_MAX_LEN,
    DEFAULT_RECEIPT_CACHE_MAX_LEN,
};

/// Parameters to configure RPC state cache.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[clap(next_help_heading = "RPC State Cache")]

pub struct RpcStateCacheArgs {
    /// Max number of blocks in cache.
    #[arg(
        long = "rpc-cache.max-blocks",
        default_value_t = DEFAULT_BLOCK_CACHE_MAX_LEN,
        value_parser = RangedU64ValueParser::<u32>::new().range(1..)
    )]
    pub max_blocks: u32,

    /// Max number receipts in cache.
    #[arg(
        long = "rpc-cache.max-receipts",
        default_value_t = DEFAULT_RECEIPT_CACHE_MAX_LEN,
        value_parser = RangedU64ValueParser::<u32>::new().range(1..)
    )]
    pub max_receipts: u32,

    /// Max number of bytes for cached env data.
    #[arg(
        long = "rpc-cache.max-envs",
        default_value_t = DEFAULT_ENV_CACHE_MAX_LEN,
        value_parser = RangedU64ValueParser::<u32>::new().range(1..)
    )]
    pub max_envs: u32,

    /// Max number of concurrent database requests.
    #[arg(
        long = "rpc-cache.max-concurrent-db-requests",
        default_value_t = DEFAULT_CONCURRENT_DB_REQUESTS,
        value_parser = RangedU64ValueParser::<usize>::new().range(1..)
    )]
    pub max_concurrent_db_requests: usize,
}

impl Default for RpcStateCacheArgs {
    fn default() -> Self {
        Self {
            max_blocks: DEFAULT_BLOCK_CACHE_MAX_LEN,
            max_receipts: DEFAULT_RECEIPT_CACHE_MAX_LEN,
            max_envs: DEFAULT_ENV_CACHE_MAX_LEN,
            max_concurrent_db_requests: DEFAULT_CONCURRENT_DB_REQUESTS,
        }
    }
}
