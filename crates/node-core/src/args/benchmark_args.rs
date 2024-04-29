//! clap [Args](clap::Args) for benchmark configuration

use clap::Args;
use humantime::{parse_duration, Duration};
use std::path::PathBuf;

/// Parameters for benchmark configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone)]
#[command(next_help_heading = "Benchmark")]
pub struct BenchmarkArgs {
    /// Run the benchmark as a continuous stream of payloads, until the benchmark is interrupted.
    #[arg(long, verbatim_doc_comment)]
    pub continuous: bool,

    /// Run the benchmark from a specific block.
    ///
    /// If the `from` block is not provided, the benchmark will start from the latest block.
    #[arg(long = "benchmark.from", verbatim_doc_comment)]
    pub from: Option<u64>,

    /// Run the benchmark to a specific block.
    ///
    /// This is mutually exclusive with `continuous`.
    #[arg(long = "benchmark.to", verbatim_doc_comment, conflicts_with = "continuous")]
    pub to: Option<u64>,

    /// Interval between blocks.
    ///
    /// Parses strings using [humantime::parse_duration]
    /// --benchmark.block-time 12s
    #[arg(
        long = "benchmark.block-time",
        value_parser = parse_duration,
        verbatim_doc_comment
    )]
    pub block_time: Option<Duration>,

    /// Whether or not to use getPayload for payload construction instead of directly converting
    /// blocks into payloads.
    #[arg(long = "benchmark.build_blocks", verbatim_doc_comment)]
    pub build_block: bool,

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    ///
    /// This will perform JWT authentication for all requests to the other EL.
    ///
    /// If no path is provided, a secret will be generated and stored in the datadir under
    /// `<DIR>/<CHAIN_ID>/jwt.hex`. For mainnet this would be `~/.reth/mainnet/jwt.hex` by default.
    #[arg(long = "benchmark.jwtsecret", value_name = "PATH", global = true, required = false)]
    pub auth_jwtsecret: Option<PathBuf>,

    /// The RPC url to use for sending engine requests.
    #[arg(
        long,
        value_name = "ENGINE_RPC_URL",
        verbatim_doc_comment,
        default_value = "http://localhost:8551"
    )]
    engine_rpc_url: String,
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
        let args = CommandParser::<BenchmarkArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
