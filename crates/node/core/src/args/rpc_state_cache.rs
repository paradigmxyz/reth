use super::database::parse_byte_size;
use clap::Args;
use reth_rpc_server_types::constants::cache::{
    DEFAULT_BAL_CACHE_MAX_BYTES, DEFAULT_BAL_CACHE_MAX_LEN, DEFAULT_BLOCK_CACHE_MAX_BYTES,
    DEFAULT_BLOCK_CACHE_MAX_LEN, DEFAULT_CACHE_IDLE_TIMEOUT, DEFAULT_CONCURRENT_DB_REQUESTS,
    DEFAULT_HEADER_CACHE_MAX_LEN, DEFAULT_MAX_CACHED_TX_HASHES, DEFAULT_RECEIPT_CACHE_MAX_BYTES,
    DEFAULT_RECEIPT_CACHE_MAX_LEN,
};
use std::time::Duration;

/// Parameters to configure RPC state cache.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "RPC State Cache")]
pub struct RpcStateCacheArgs {
    /// Max number of blocks in cache.
    #[arg(
        long = "rpc-cache.max-blocks",
        default_value_t = DEFAULT_BLOCK_CACHE_MAX_LEN,
    )]
    pub max_blocks: u32,

    /// Max number receipts in cache.
    #[arg(
        long = "rpc-cache.max-receipts",
        default_value_t = DEFAULT_RECEIPT_CACHE_MAX_LEN,
    )]
    pub max_receipts: u32,

    /// Legacy no-op retained for CLI compatibility.
    #[arg(
        long = "rpc-cache.max-headers",
        alias = "rpc-cache.max-envs",
        default_value_t = DEFAULT_HEADER_CACHE_MAX_LEN,
        hide = true,
    )]
    pub max_headers: u32,

    /// Max number of block access lists in cache.
    #[arg(
        long = "rpc-cache.max-bals",
        default_value_t = DEFAULT_BAL_CACHE_MAX_LEN,
    )]
    pub max_bals: u32,

    /// Maximum estimated block cache memory in bytes or with a unit (e.g. 1GB, 500MB).
    /// Units use powers of 1024. Zero disables caching. The entry count limit also applies.
    #[arg(long = "rpc-cache.max-blocks-bytes", value_name = "BYTES", value_parser = parse_byte_size, default_value_t = DEFAULT_BLOCK_CACHE_MAX_BYTES)]
    pub max_blocks_bytes: usize,

    /// Maximum estimated receipts cache memory in bytes or with a unit (e.g. 1GB, 500MB).
    /// Units use powers of 1024. Zero disables caching. The entry count limit also applies.
    #[arg(long = "rpc-cache.max-receipts-bytes", value_name = "BYTES", value_parser = parse_byte_size, default_value_t = DEFAULT_RECEIPT_CACHE_MAX_BYTES)]
    pub max_receipts_bytes: usize,

    /// Maximum estimated block access list cache memory in bytes or with a unit (e.g. 1GB, 500MB).
    /// Units use powers of 1024. Zero disables caching. The entry count limit also applies.
    #[arg(long = "rpc-cache.max-bals-bytes", value_name = "BYTES", value_parser = parse_byte_size, default_value_t = DEFAULT_BAL_CACHE_MAX_BYTES)]
    pub max_bals_bytes: usize,

    /// Evict blocks, receipts, and block access lists after this duration without a cache hit
    /// (e.g. 5m, 30s). Zero disables expiration. Cleanup starts every 1 to 60 seconds and removes
    /// at most five entries per cache per poll.
    #[arg(long = "rpc-cache.idle-timeout", value_name = "DURATION", value_parser = humantime::parse_duration, default_value = "1h")]
    pub idle_timeout: Duration,

    /// Cache block access lists computed by RPC requests for transaction tracing.
    #[arg(long = "rpc-cache.cache-computed-bals")]
    pub cache_computed_bals: bool,

    /// Replay new canonical blocks to cache block access lists until native BAL support.
    ///
    /// Optionally replay the latest BLOCKS blocks sequentially on startup. Without a count, only
    /// new blocks are prewarmed. Implies --rpc-cache.cache-computed-bals.
    /// Prewarming stops when a canonical block with a block access list hash is received.
    #[arg(
        long = "rpc-cache.prewarm-bals",
        value_name = "BLOCKS",
        num_args = 0..=1,
        default_missing_value = "0",
        require_equals = true,
    )]
    pub prewarm_bals: Option<usize>,

    /// Max number of concurrent database requests.
    #[arg(
        long = "rpc-cache.max-concurrent-db-requests",
        default_value_t = DEFAULT_CONCURRENT_DB_REQUESTS,
    )]
    pub max_concurrent_db_requests: usize,

    /// Maximum number of transaction hashes to cache for transaction lookups.
    #[arg(
        long = "rpc-cache.max-cached-tx-hashes",
        default_value_t = DEFAULT_MAX_CACHED_TX_HASHES,
    )]
    pub max_cached_tx_hashes: u32,
}

impl RpcStateCacheArgs {
    /// Sets the Cache sizes to zero, effectively disabling caching.
    pub const fn set_zero_lengths(&mut self) {
        self.max_blocks = 0;
        self.max_receipts = 0;
        self.max_headers = 0;
        self.max_bals = 0;
        self.max_blocks_bytes = 0;
        self.max_receipts_bytes = 0;
        self.max_bals_bytes = 0;
        self.idle_timeout = Duration::ZERO;
        self.cache_computed_bals = false;
        self.prewarm_bals = None;
    }
}

impl Default for RpcStateCacheArgs {
    fn default() -> Self {
        Self {
            max_blocks: DEFAULT_BLOCK_CACHE_MAX_LEN,
            max_receipts: DEFAULT_RECEIPT_CACHE_MAX_LEN,
            max_headers: DEFAULT_HEADER_CACHE_MAX_LEN,
            max_bals: DEFAULT_BAL_CACHE_MAX_LEN,
            max_blocks_bytes: DEFAULT_BLOCK_CACHE_MAX_BYTES,
            max_receipts_bytes: DEFAULT_RECEIPT_CACHE_MAX_BYTES,
            max_bals_bytes: DEFAULT_BAL_CACHE_MAX_BYTES,
            idle_timeout: DEFAULT_CACHE_IDLE_TIMEOUT,
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
    use crate::node_config::NodeConfig;
    use clap::Parser;

    #[derive(Parser)]
    struct CommandParser {
        #[command(flatten)]
        args: RpcStateCacheArgs,
    }

    #[test]
    fn rpc_cache_defaults() {
        let args = CommandParser::parse_from(["reth"]).args;
        assert_eq!(args, RpcStateCacheArgs::default());

        for flag in ["--rpc-cache.max-headers", "--rpc-cache.max-envs"] {
            let args = CommandParser::parse_from(["reth", flag, "123"]).args;
            assert_eq!(args.max_headers, 123);
        }
    }

    #[test]
    fn rpc_cache_byte_limits_accept_zero_and_maximum() {
        for limit in [0, 1024, usize::MAX] {
            let value = limit.to_string();
            let args = CommandParser::parse_from([
                "reth",
                "--rpc-cache.max-blocks-bytes",
                &value,
                "--rpc-cache.max-receipts-bytes",
                &value,
                "--rpc-cache.max-bals-bytes",
                &value,
            ])
            .args;
            assert_eq!(args.max_blocks_bytes, limit);
            assert_eq!(args.max_receipts_bytes, limit);
            assert_eq!(args.max_bals_bytes, limit);
        }
    }

    #[test]
    fn rpc_cache_byte_limits_reject_invalid_values() {
        let overflow = format!("{}0", usize::MAX);
        let unit_overflow = format!("{}GB", usize::MAX);
        for flag in [
            "--rpc-cache.max-blocks-bytes",
            "--rpc-cache.max-receipts-bytes",
            "--rpc-cache.max-bals-bytes",
        ] {
            for value in ["-1", "-1GB", "1XB", "1.5GB", &overflow, &unit_overflow] {
                assert!(
                    CommandParser::try_parse_from(["reth", &format!("{flag}={value}")]).is_err(),
                    "accepted {flag}={value}"
                );
            }
        }
    }

    #[test]
    fn rpc_cache_byte_limits_accept_units() {
        for (value, expected) in [
            ("0GB", 0),
            ("1024B", 1024),
            ("1KB", 1024),
            ("500MB", 500 * 1024 * 1024),
            ("1GB", 1024 * 1024 * 1024),
            ("1gb", 1024 * 1024 * 1024),
            (" 500 MB ", 500 * 1024 * 1024),
            ("1TB", 1024usize.pow(4)),
        ] {
            let args = CommandParser::parse_from([
                "reth",
                "--rpc-cache.max-blocks-bytes",
                value,
                "--rpc-cache.max-receipts-bytes",
                value,
                "--rpc-cache.max-bals-bytes",
                value,
            ])
            .args;
            assert_eq!(args.max_blocks_bytes, expected);
            assert_eq!(args.max_receipts_bytes, expected);
            assert_eq!(args.max_bals_bytes, expected);
        }
    }

    #[test]
    fn rpc_cache_idle_timeout_parses_durations() {
        for (value, expected) in [
            ("0s", Duration::ZERO),
            ("500ms", Duration::from_millis(500)),
            ("30s", Duration::from_secs(30)),
            ("5m", Duration::from_secs(300)),
        ] {
            let args = CommandParser::parse_from(["reth", "--rpc-cache.idle-timeout", value]).args;
            assert_eq!(args.idle_timeout, expected);
        }
        for value in ["-1s", "invalid", "18446744073709551616s"] {
            assert!(CommandParser::try_parse_from([
                "reth",
                &format!("--rpc-cache.idle-timeout={value}")
            ])
            .is_err());
        }
    }

    #[test]
    fn disabling_rpc_cache_overrides_positive_byte_limits() {
        let mut config = NodeConfig::default();
        config.rpc.rpc_state_cache = RpcStateCacheArgs {
            max_blocks_bytes: 1024,
            max_receipts_bytes: 2048,
            max_bals_bytes: 4096,
            idle_timeout: Duration::from_secs(60),
            cache_computed_bals: true,
            prewarm_bals: Some(10),
            ..Default::default()
        };

        let args = config.with_disabled_rpc_cache().rpc.rpc_state_cache;
        assert_eq!(args.max_blocks, 0);
        assert_eq!(args.max_receipts, 0);
        assert_eq!(args.max_bals, 0);
        assert_eq!(args.max_blocks_bytes, 0);
        assert_eq!(args.max_receipts_bytes, 0);
        assert_eq!(args.max_bals_bytes, 0);
        assert_eq!(args.idle_timeout, Duration::ZERO);
        assert!(!args.cache_computed_bals);
        assert_eq!(args.prewarm_bals, None);
    }
}
