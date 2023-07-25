//! clap [Args](clap::Args) for Dev testnet configuration
use std::time::Duration;

use clap::Args;
use humantime::parse_duration;

/// Parameters for Dev testnet configuration
#[derive(Debug, Args, PartialEq, Default, Clone, Copy)]
#[command(next_help_heading = "Dev testnet")]
pub struct DevArgs {
    /// Start the node in dev mode
    ///
    /// This mode uses a local proof-of-authority consensus engine with either fixed block times
    /// or automatically mined blocks.
    /// Disables network discovery and enables local http server.
    /// Prefunds 20 accounts derived by mnemonic "test test test test test test test test test test
    /// test junk" with 10 000 ETH each.
    #[arg(long = "dev", alias = "auto-mine", help_heading = "Dev testnet", verbatim_doc_comment)]
    pub dev: bool,

    /// How many transactions to mine per block.
    #[arg(
        long = "dev.block_max_transactions",
        help_heading = "Dev testnet",
        conflicts_with = "block_time"
    )]
    pub block_max_transactions: Option<usize>,

    /// Interval between blocks.
    ///
    /// Parses strings using [humantime::parse_duration]
    /// --dev.block_time 12s
    #[arg(
        long = "dev.block_time",
        help_heading = "Dev testnet", 
        conflicts_with = "block_max_transactions",
        value_parser = parse_duration,
        verbatim_doc_comment
    )]
    pub block_time: Option<Duration>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_parse_dev_args() {
        let args = CommandParser::<DevArgs>::parse_from(["reth"]).args;
        assert_eq!(args, DevArgs { dev: false, block_max_transactions: None, block_time: None });

        let args = CommandParser::<DevArgs>::parse_from(["reth", "--dev"]).args;
        assert_eq!(args, DevArgs { dev: true, block_max_transactions: None, block_time: None });

        let args = CommandParser::<DevArgs>::parse_from(["reth", "--auto-mine"]).args;
        assert_eq!(args, DevArgs { dev: true, block_max_transactions: None, block_time: None });

        let args = CommandParser::<DevArgs>::parse_from([
            "reth",
            "--dev",
            "--dev.block_max_transactions",
            "2",
        ])
        .args;
        assert_eq!(args, DevArgs { dev: true, block_max_transactions: Some(2), block_time: None });

        let args =
            CommandParser::<DevArgs>::parse_from(["reth", "--dev", "--dev.block_time", "1s"]).args;
        assert_eq!(
            args,
            DevArgs {
                dev: true,
                block_max_transactions: None,
                block_time: Some(std::time::Duration::from_secs(1))
            }
        );
    }

    #[test]
    fn test_parse_dev_args_conflicts() {
        let args = CommandParser::<DevArgs>::try_parse_from([
            "reth",
            "--dev",
            "--dev.block_max_transactions",
            "2",
            "--dev.block_time",
            "1s",
        ]);
        assert!(args.is_err());
    }
}
