//! clap [Args](clap::Args) for benchmark configuration

use clap::Args;
use std::path::PathBuf;

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

    /// Path(s) to JWT secret(s) to use for the authenticated engine-API RPC server(s).
    ///
    /// This will perform JWT authentication for all requests to the given engine RPC url(s).
    /// When multiple engine-rpc-urls are provided, you can either:
    /// - Provide one JWT secret (used for all engine URLs)
    /// - Provide the same number of JWT secrets (one per engine URL, in order)
    #[arg(
        long = "jwt-secret",
        alias = "jwtsecret",
        value_name = "PATH",
        global = true,
        required = false,
        action = clap::ArgAction::Append
    )]
    pub auth_jwtsecret: Vec<PathBuf>,

    /// The RPC url(s) to use for sending engine requests.
    ///
    /// Can be specified multiple times to broadcast payloads to multiple nodes.
    /// All nodes must be at the same block height before benchmarking begins.
    #[arg(
        long,
        value_name = "ENGINE_RPC_URL",
        verbatim_doc_comment,
        default_value = "http://localhost:8551",
        action = clap::ArgAction::Append
    )]
    pub engine_rpc_url: Vec<String>,

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
            engine_rpc_url: vec!["http://localhost:8551".to_string()],
            ..Default::default()
        };
        let args = CommandParser::<BenchmarkArgs>::parse_from(["reth-bench"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn test_parse_multiple_engine_urls() {
        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--engine-rpc-url",
            "http://localhost:8551",
            "--engine-rpc-url",
            "http://localhost:8552",
        ])
        .args;
        assert_eq!(
            args.engine_rpc_url,
            vec!["http://localhost:8551".to_string(), "http://localhost:8552".to_string()]
        );
    }

    #[test]
    fn test_parse_multiple_jwt_secrets() {
        let args = CommandParser::<BenchmarkArgs>::parse_from([
            "reth-bench",
            "--jwt-secret",
            "/path/to/jwt1.hex",
            "--jwt-secret",
            "/path/to/jwt2.hex",
        ])
        .args;
        assert_eq!(
            args.auth_jwtsecret,
            vec![PathBuf::from("/path/to/jwt1.hex"), PathBuf::from("/path/to/jwt2.hex")]
        );
    }
}
