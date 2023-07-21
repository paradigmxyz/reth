//! clap [Args](clap::Args) for dev testnet configuration
use std::time::Duration;

use clap::Args;
use humantime::parse_duration;

/// Parameters for database configuration
#[derive(Debug, Args, PartialEq, Default, Clone, Copy)]
#[command(next_help_heading = "Dev")]
pub struct DevArgs {
    /// Start the node in ephemeral dev mode
    ///
    /// This will
    #[arg(long = "dev", help_heading = "Dev")]
    pub dev: bool,

    /// How many block_max_transactions to mine per block.
    #[arg(
        long = "dev.block_max_transactions",
        help_heading = "Dev",
        conflicts_with = "block_time"
    )]
    pub block_max_transactions: Option<usize>,

    /// Interval between blocks.
    #[arg(
        long = "dev.block_time",
        help_heading = "Dev", 
        conflicts_with = "block_max_transactions",
        value_parser = parse_duration
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
                block_time: Some(std::time::Duration::from_secs(1).into())
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
