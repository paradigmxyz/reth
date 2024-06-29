//! clap [Args](clap::Args) for debugging purposes

use clap::Args;
use reth_primitives::B256;
use std::path::PathBuf;

/// Parameters for debugging purposes
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Debug")]
pub struct DebugArgs {
    /// Flag indicating whether the node should be terminated after the pipeline sync.
    #[arg(long = "debug.terminate", help_heading = "Debug")]
    pub terminate: bool,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    pub tip: Option<B256>,

    /// Runs the sync only up to the specified block.
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    pub max_block: Option<u64>,

    /// Runs a fake consensus client that advances the chain using recent block hashes
    /// on Etherscan. If specified, requires an `ETHERSCAN_API_KEY` environment variable.
    #[arg(
        long = "debug.etherscan",
        help_heading = "Debug",
        conflicts_with = "tip",
        conflicts_with = "rpc_consensus_ws",
        value_name = "ETHERSCAN_API_URL"
    )]
    pub etherscan: Option<Option<String>>,

    /// Runs a fake consensus client using blocks fetched from an RPC `WebSocket` endpoint.
    #[arg(
        long = "debug.rpc-consensus-ws",
        help_heading = "Debug",
        conflicts_with = "tip",
        conflicts_with = "etherscan"
    )]
    pub rpc_consensus_ws: Option<String>,

    /// If provided, the engine will skip `n` consecutive FCUs.
    #[arg(long = "debug.skip-fcu", help_heading = "Debug")]
    pub skip_fcu: Option<usize>,

    /// If provided, the engine will skip `n` consecutive new payloads.
    #[arg(long = "debug.skip-new-payload", help_heading = "Debug")]
    pub skip_new_payload: Option<usize>,

    /// The path to store engine API messages at.
    /// If specified, all of the intercepted engine API messages
    /// will be written to specified location.
    #[arg(long = "debug.engine-api-store", help_heading = "Debug", value_name = "PATH")]
    pub engine_api_store: Option<PathBuf>,
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
    fn test_parse_database_args() {
        let default_args = DebugArgs::default();
        let args = CommandParser::<DebugArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
