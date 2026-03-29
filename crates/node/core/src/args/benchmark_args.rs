//! clap [Args](clap::Args) for benchmark configuration

use clap::Args;
use std::{path::PathBuf, str::FromStr};

/// Parameters for benchmark configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone)]
#[command(next_help_heading = "Benchmark")]
pub struct BenchmarkArgs {
    /// Run the benchmark from a specific block.
    #[arg(long, verbatim_doc_comment)]
    pub from: Option<u64>,

    /// Run the benchmark to a specific block.
    #[arg(long, verbatim_doc_comment)]
    pub to: Option<u64>,

    /// Number of blocks to advance from the current head block.
    /// When specified, automatically sets --from to current head + 1 and --to to current head +
    /// advance. Cannot be used together with explicit --from and --to arguments.
    #[arg(long, conflicts_with_all = &["from", "to"], verbatim_doc_comment)]
    pub advance: Option<u64>,

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    ///
    /// This will perform JWT authentication for all requests to the given engine RPC url.
    ///
    /// If no path is provided, a secret will be generated and stored in the datadir under
    /// `<DIR>/<CHAIN_ID>/jwt.hex`. For mainnet this would be `~/.local/share/reth/mainnet/jwt.hex`
    /// by default.
    #[arg(
        long = "jwt-secret",
        alias = "jwtsecret",
        value_name = "PATH",
        global = true,
        required = false
    )]
    pub auth_jwtsecret: Option<PathBuf>,

    /// The RPC url to use for sending engine requests.
    #[arg(
        long,
        value_name = "ENGINE_RPC_URL",
        verbatim_doc_comment,
        default_value = "http://localhost:8551"
    )]
    pub engine_rpc_url: String,

    /// The `WebSocket` RPC URL to use for persistence subscriptions.
    ///
    /// If not provided, will attempt to derive from engine-rpc-url by:
    /// - Converting http/https to ws/wss
    /// - Using port 8546 (standard RPC `WebSocket` port)
    ///
    /// Example: `ws://localhost:8546`
    #[arg(long, value_name = "WS_RPC_URL", verbatim_doc_comment)]
    pub ws_rpc_url: Option<String>,

    /// The path to the output directory for granular benchmark results.
    #[arg(long, short, value_name = "BENCHMARK_OUTPUT", verbatim_doc_comment)]
    pub output: Option<PathBuf>,

    /// Optional Prometheus metrics endpoint to scrape after each block.
    ///
    /// When provided, reth-bench will fetch metrics from this URL after each
    /// `newPayload` / `forkchoiceUpdated` call, recording per-block execution
    /// and state root durations. Results are written to `metrics.csv` in the
    /// output directory.
    ///
    /// Example: `http://127.0.0.1:9001/metrics`
    #[arg(long = "metrics-url", value_name = "URL", verbatim_doc_comment)]
    pub metrics_url: Option<String>,

    /// Number of retries for fetching blocks from `--rpc-url` after a failure.
    ///
    /// Use `0` to fail immediately, or `forever` to never stop retrying.
    #[arg(
        long = "rpc-block-fetch-retries",
        value_name = "RETRIES",
        default_value = "10",
        value_parser = parse_rpc_block_fetch_retries,
        verbatim_doc_comment
    )]
    pub rpc_block_fetch_retries: RpcBlockFetchRetries,

    /// Use `reth_newPayload` endpoint instead of `engine_newPayload*`.
    ///
    /// The `reth_newPayload` endpoint is a reth-specific extension that takes `ExecutionData`
    /// directly, waits for persistence and cache updates to complete before processing,
    /// and returns server-side timing breakdowns (latency, persistence wait, cache wait).
    ///
    /// Cannot be used with `--wait-for-persistence` because `reth_newPayload` already
    /// waits for persistence by default.
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    pub reth_new_payload: bool,

    /// Control when `reth_newPayload` waits for in-flight persistence.
    ///
    /// Accepts `always` (wait on every block), `never` (default), or a number N
    /// to wait every N blocks and skip the rest.
    ///
    /// Requires `--reth-new-payload`.
    #[arg(
        long = "wait-for-persistence",
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "always",
        value_parser = clap::value_parser!(WaitForPersistence),
        requires = "reth_new_payload",
        verbatim_doc_comment
    )]
    pub wait_for_persistence: Option<WaitForPersistence>,

    /// Skip waiting for execution cache and sparse trie locks before processing.
    ///
    /// Only works with `--reth-new-payload`. When set, passes `wait_for_caches: false`
    /// to the `reth_newPayload` endpoint.
    #[arg(long, default_value = "false", verbatim_doc_comment, requires = "reth_new_payload")]
    pub no_wait_for_caches: bool,

    /// Fetch and replay RLP-encoded blocks. Implies `reth_new_payload`.
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    pub rlp_blocks: bool,
}

/// Retry strategy for fetching blocks from the benchmark RPC provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcBlockFetchRetries {
    /// Retry up to `u32` times after the first failed attempt.
    Finite(u32),
    /// Retry forever.
    Forever,
}

impl RpcBlockFetchRetries {
    /// Returns the maximum number of retries for the `RetryBackoffLayer`.
    pub const fn as_max_retries(self) -> u32 {
        match self {
            Self::Finite(n) => n,
            Self::Forever => u32::MAX,
        }
    }
}

impl Default for RpcBlockFetchRetries {
    fn default() -> Self {
        Self::Finite(10)
    }
}

impl FromStr for RpcBlockFetchRetries {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("forever") ||
            s.eq_ignore_ascii_case("infinite") ||
            s.eq_ignore_ascii_case("inf")
        {
            return Ok(Self::Forever)
        }

        let retries = s
            .parse::<u32>()
            .map_err(|_| format!("invalid retry value {s:?}, expected a number or 'forever'"))?;
        Ok(Self::Finite(retries))
    }
}

fn parse_rpc_block_fetch_retries(value: &str) -> Result<RpcBlockFetchRetries, String> {
    value.parse()
}

/// Controls when `reth_newPayload` waits for in-flight persistence to complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitForPersistence {
    /// Wait for persistence on every block (default `reth_newPayload` behavior).
    #[default]
    Always,
    /// Never wait for persistence.
    Never,
    /// Wait for persistence every N blocks, skip for the rest.
    EveryN(u64),
}

impl WaitForPersistence {
    /// Returns the `wait_for_persistence` RPC parameter value for a given block number.
    ///
    /// - `None` → use the server default (true)
    /// - `Some(false)` → skip waiting
    /// - `Some(true)` → explicitly wait
    pub const fn rpc_value(self, block_number: u64) -> Option<bool> {
        match self {
            Self::Always => None,
            Self::Never => Some(false),
            Self::EveryN(n) => {
                if block_number.is_multiple_of(n) {
                    Some(true)
                } else {
                    Some(false)
                }
            }
        }
    }
}

impl FromStr for WaitForPersistence {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("always") {
            return Ok(Self::Always)
        }
        if s.eq_ignore_ascii_case("never") {
            return Ok(Self::Never)
        }
        let n = s.parse::<u64>().map_err(|_| {
            format!("invalid value {s:?}, expected 'always', 'never', or a block interval number")
        })?;
        if n == 0 {
            return Err("block interval must be > 0, use 'never' to disable".to_string())
        }
        Ok(Self::EveryN(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_parse_benchmark_args() {
        let default_args = BenchmarkArgs {
            engine_rpc_url: "http://localhost:8551".to_string(),
            ..Default::default()
        };
        let args = CommandParser::<BenchmarkArgs>::parse_from(["reth-bench"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn test_parse_rpc_block_fetch_retries_forever() {
        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--rpc-block-fetch-retries",
            "forever",
        ])
        .args;
        assert_eq!(args.rpc_block_fetch_retries, RpcBlockFetchRetries::Forever);
    }

    #[test]
    fn test_parse_rpc_block_fetch_retries_number() {
        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--rpc-block-fetch-retries",
            "7",
        ])
        .args;
        assert_eq!(args.rpc_block_fetch_retries, RpcBlockFetchRetries::Finite(7));
    }

    #[test]
    fn test_parse_wait_for_persistence() {
        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--reth-new-payload",
            "--wait-for-persistence",
            "always",
        ])
        .args;
        assert_eq!(args.wait_for_persistence, Some(WaitForPersistence::Always));

        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--reth-new-payload",
            "--wait-for-persistence",
            "never",
        ])
        .args;
        assert_eq!(args.wait_for_persistence, Some(WaitForPersistence::Never));

        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--reth-new-payload",
            "--wait-for-persistence",
            "10",
        ])
        .args;
        assert_eq!(args.wait_for_persistence, Some(WaitForPersistence::EveryN(10)));

        // bare --wait-for-persistence (no value) defaults to Always
        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--reth-new-payload",
            "--wait-for-persistence",
        ])
        .args;
        assert_eq!(args.wait_for_persistence, Some(WaitForPersistence::Always));

        // default is None
        let args = CommandParser::<BenchmarkArgs>::parse_from(["reth-bench"]).args;
        assert_eq!(args.wait_for_persistence, None);
    }
}
