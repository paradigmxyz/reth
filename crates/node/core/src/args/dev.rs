//! clap [Args](clap::Args) for Dev testnet configuration

use std::time::Duration;

use clap::Args;
use humantime::parse_duration;

const DEFAULT_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// Parameters for Dev testnet configuration
#[derive(Debug, Args, PartialEq, Eq, Clone)]
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
        long = "dev.block-max-transactions",
        help_heading = "Dev testnet",
        conflicts_with = "block_time"
    )]
    pub block_max_transactions: Option<usize>,

    /// Interval between blocks.
    ///
    /// Parses strings using [`humantime::parse_duration`]
    /// --dev.block-time 12s
    #[arg(
        long = "dev.block-time",
        help_heading = "Dev testnet",
        conflicts_with = "block_max_transactions",
        value_parser = parse_duration,
        verbatim_doc_comment
    )]
    pub block_time: Option<Duration>,

    /// Derive dev accounts from a fixed mnemonic instead of random ones.
    #[arg(
        long = "dev.mnemonic",
        help_heading = "Dev testnet",
        value_name = "MNEMONIC",
        requires = "dev",
        verbatim_doc_comment,
        default_value = DEFAULT_MNEMONIC
    )]
    pub dev_mnemonic: String,
}

impl Default for DevArgs {
    fn default() -> Self {
        Self {
            dev: false,
            block_max_transactions: None,
            block_time: None,
            dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
        }
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
    fn test_parse_dev_args() {
        let args = CommandParser::<DevArgs>::parse_from(["reth"]).args;
        assert_eq!(
            args,
            DevArgs {
                dev: false,
                block_max_transactions: None,
                block_time: None,
                dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
            }
        );

        let args = CommandParser::<DevArgs>::parse_from(["reth", "--dev"]).args;
        assert_eq!(
            args,
            DevArgs {
                dev: true,
                block_max_transactions: None,
                block_time: None,
                dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
            }
        );

        let args = CommandParser::<DevArgs>::parse_from(["reth", "--auto-mine"]).args;
        assert_eq!(
            args,
            DevArgs {
                dev: true,
                block_max_transactions: None,
                block_time: None,
                dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
            }
        );

        let args = CommandParser::<DevArgs>::parse_from([
            "reth",
            "--dev",
            "--dev.block-max-transactions",
            "2",
        ])
        .args;
        assert_eq!(
            args,
            DevArgs {
                dev: true,
                block_max_transactions: Some(2),
                block_time: None,
                dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
            }
        );

        let args =
            CommandParser::<DevArgs>::parse_from(["reth", "--dev", "--dev.block-time", "1s"]).args;
        assert_eq!(
            args,
            DevArgs {
                dev: true,
                block_max_transactions: None,
                block_time: Some(std::time::Duration::from_secs(1)),
                dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
            }
        );
    }

    #[test]
    fn test_parse_dev_args_conflicts() {
        let args = CommandParser::<DevArgs>::try_parse_from([
            "reth",
            "--dev",
            "--dev.block-max-transactions",
            "2",
            "--dev.block-time",
            "1s",
        ]);
        assert!(args.is_err());
    }

    #[test]
    fn dev_args_default_sanity_check() {
        let default_args = DevArgs::default();
        let args = CommandParser::<DevArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
