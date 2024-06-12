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

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    ///
    /// This will perform JWT authentication for all requests to the given engine RPC url.
    ///
    /// If no path is provided, a secret will be generated and stored in the datadir under
    /// `<DIR>/<CHAIN_ID>/jwt.hex`. For mainnet this would be `~/.reth/mainnet/jwt.hex` by default.
    #[arg(long = "jwtsecret", value_name = "PATH", global = true, required = false)]
    pub auth_jwtsecret: Option<PathBuf>,

    /// The RPC url to use for sending engine requests.
    #[arg(
        long,
        value_name = "ENGINE_RPC_URL",
        verbatim_doc_comment,
        default_value = "http://localhost:8551"
    )]
    pub engine_rpc_url: String,

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
            engine_rpc_url: "http://localhost:8551".to_string(),
            ..Default::default()
        };
        let args = CommandParser::<BenchmarkArgs>::parse_from(["reth-bench"]).args;
        assert_eq!(args, default_args);
    }
}
