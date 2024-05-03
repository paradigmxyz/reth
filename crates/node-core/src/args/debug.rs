//! clap [Args](clap::Args) for debugging purposes

use clap::Args;
use reth_primitives::B256;
use std::path::PathBuf;

/// Parameters for debugging purposes
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Debug")]
pub struct DebugArgs {
    /// Prompt the downloader to download blocks one at a time.
    ///
    /// NOTE: This is for testing purposes only.
    #[arg(long = "debug.continuous", help_heading = "Debug", conflicts_with = "tip")]
    pub continuous: bool,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    #[arg(long = "debug.terminate", help_heading = "Debug")]
    pub terminate: bool,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug", conflicts_with = "continuous")]
    pub tip: Option<B256>,

    /// Runs the sync only up to the specified block.
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    pub max_block: Option<u64>,

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
