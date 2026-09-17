//! clap [Args](clap::Args) for Dev testnet configuration

use std::{num::NonZeroUsize, sync::OnceLock, time::Duration};

use clap::{builder::Resettable, Args};
use humantime::{format_duration, parse_duration};
use reth_engine_local::DEFAULT_FINALITY_DEPTH;

const DEFAULT_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// Global static dev testnet defaults
static DEV_DEFAULTS: OnceLock<DefaultDevArgs> = OnceLock::new();

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
    #[arg(long = "dev", alias = "auto-mine", help_heading = "Dev testnet", default_value_t = DefaultDevArgs::get_global().dev, verbatim_doc_comment)]
    pub dev: bool,

    /// How many transactions to mine per block.
    #[arg(
        long = "dev.block-max-transactions",
        help_heading = "Dev testnet",
        conflicts_with = "block_time",
        default_value = Resettable::from(DefaultDevArgs::get_global().block_max_transactions.map(|v| v.to_string().into()))
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
        default_value = Resettable::from(DefaultDevArgs::get_global().block_time.map(|v| format_duration(v).to_string().into())),
        verbatim_doc_comment
    )]
    pub block_time: Option<Duration>,

    /// Number of confirmations required before a block is finalized.
    ///
    /// A depth of `1` finalizes the canonical head immediately.
    #[arg(
        long = "dev.finality-depth",
        help_heading = "Dev testnet",
        default_value_t = DefaultDevArgs::get_global().finality_depth,
        verbatim_doc_comment
    )]
    pub finality_depth: NonZeroUsize,

    /// Time to wait after initiating payload building before resolving.
    ///
    /// Introduces a sleep between `fork_choice_updated` and `resolve_kind` in the
    /// local miner, giving the payload job time for multiple rebuild attempts with
    /// new transactions from the pool.
    ///
    /// Parses strings using [`humantime::parse_duration`]
    /// --dev.payload-wait-time 450ms
    #[arg(
        long = "dev.payload-wait-time",
        help_heading = "Dev testnet",
        value_parser = parse_duration,
        default_value = Resettable::from(DefaultDevArgs::get_global().payload_wait_time.map(|v| format_duration(v).to_string().into())),
        verbatim_doc_comment
    )]
    pub payload_wait_time: Option<Duration>,

    /// Derive dev accounts from a fixed mnemonic instead of random ones.
    #[arg(
        long = "dev.mnemonic",
        help_heading = "Dev testnet",
        value_name = "MNEMONIC",
        requires = "dev",
        verbatim_doc_comment,
        default_value_t = DefaultDevArgs::get_global().dev_mnemonic.clone()
    )]
    pub dev_mnemonic: String,
}

/// Default values for dev testnet CLI arguments that can be customized.
///
/// Global defaults can be set via [`DefaultDevArgs::try_init`].
#[derive(Debug, Clone)]
pub struct DefaultDevArgs {
    /// Default for `--dev`.
    pub dev: bool,
    /// Default maximum number of transactions to mine per block.
    pub block_max_transactions: Option<usize>,
    /// Default interval between blocks.
    pub block_time: Option<Duration>,
    /// Default number of confirmations required before finalization.
    pub finality_depth: NonZeroUsize,
    /// Default time to wait before resolving a payload.
    pub payload_wait_time: Option<Duration>,
    /// Default mnemonic used to derive dev accounts.
    pub dev_mnemonic: String,
}

impl DefaultDevArgs {
    /// Initialize the global dev testnet defaults with this configuration.
    pub fn try_init(self) -> Result<(), Self> {
        DEV_DEFAULTS.set(self)
    }

    /// Get a reference to the global dev testnet defaults.
    pub fn get_global() -> &'static Self {
        DEV_DEFAULTS.get_or_init(Self::default)
    }

    /// Set whether dev mode is enabled by default.
    pub const fn with_dev(mut self, dev: bool) -> Self {
        self.dev = dev;
        self
    }

    /// Set the default maximum number of transactions to mine per block.
    pub const fn with_block_max_transactions(mut self, count: Option<usize>) -> Self {
        self.block_max_transactions = count;
        self
    }

    /// Set the default interval between blocks.
    pub const fn with_block_time(mut self, block_time: Option<Duration>) -> Self {
        self.block_time = block_time;
        self
    }

    /// Set the default number of confirmations required before finalization.
    pub const fn with_finality_depth(mut self, finality_depth: NonZeroUsize) -> Self {
        self.finality_depth = finality_depth;
        self
    }

    /// Set the default time to wait before resolving a payload.
    pub const fn with_payload_wait_time(mut self, payload_wait_time: Option<Duration>) -> Self {
        self.payload_wait_time = payload_wait_time;
        self
    }

    /// Set the default mnemonic used to derive dev accounts.
    pub fn with_dev_mnemonic(mut self, dev_mnemonic: String) -> Self {
        self.dev_mnemonic = dev_mnemonic;
        self
    }
}

impl Default for DefaultDevArgs {
    fn default() -> Self {
        Self {
            dev: false,
            block_max_transactions: None,
            block_time: None,
            finality_depth: DEFAULT_FINALITY_DEPTH,
            payload_wait_time: None,
            dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
        }
    }
}

impl Default for DevArgs {
    fn default() -> Self {
        let DefaultDevArgs {
            dev,
            block_max_transactions,
            block_time,
            finality_depth,
            payload_wait_time,
            dev_mnemonic,
        } = DefaultDevArgs::get_global().clone();
        Self {
            dev,
            block_max_transactions,
            block_time,
            finality_depth,
            payload_wait_time,
            dev_mnemonic,
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
                finality_depth: DEFAULT_FINALITY_DEPTH,
                payload_wait_time: None,
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
                finality_depth: DEFAULT_FINALITY_DEPTH,
                payload_wait_time: None,
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
                finality_depth: DEFAULT_FINALITY_DEPTH,
                payload_wait_time: None,
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
                finality_depth: DEFAULT_FINALITY_DEPTH,
                payload_wait_time: None,
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
                finality_depth: DEFAULT_FINALITY_DEPTH,
                payload_wait_time: None,
                dev_mnemonic: DEFAULT_MNEMONIC.to_string(),
            }
        );

        let args =
            CommandParser::<DevArgs>::parse_from(["reth", "--dev", "--dev.finality-depth", "1"])
                .args;
        assert_eq!(args.finality_depth, NonZeroUsize::new(1).unwrap());
    }

    #[test]
    fn test_rejects_zero_finality_depth() {
        assert!(CommandParser::<DevArgs>::try_parse_from([
            "reth",
            "--dev",
            "--dev.finality-depth",
            "0",
        ])
        .is_err());
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
