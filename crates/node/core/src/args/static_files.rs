//! clap [Args](clap::Args) for static files configuration

use clap::Args;
use reth_config::config::{BlocksPerFileConfig, StaticFilesConfig};

/// Blocks per static file when running in `--minimal` node.
///
/// 10000 blocks per static file allows us to prune all history every 10k blocks.
pub const MINIMAL_BLOCKS_PER_FILE: u64 = 10000;

/// Default value for static file storage flags.
///
/// When the `edge` feature is enabled, defaults to `true` to enable edge storage features.
/// Otherwise defaults to `false` for legacy behavior.
const fn default_static_file_flag() -> bool {
    cfg!(feature = "edge")
}

/// Parameters for static files configuration
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy)]
#[command(next_help_heading = "Static Files")]
pub struct StaticFilesArgs {
    /// Number of blocks per file for the headers segment.
    #[arg(long = "static-files.blocks-per-file.headers")]
    pub blocks_per_file_headers: Option<u64>,

    /// Number of blocks per file for the transactions segment.
    #[arg(long = "static-files.blocks-per-file.transactions")]
    pub blocks_per_file_transactions: Option<u64>,

    /// Number of blocks per file for the receipts segment.
    #[arg(long = "static-files.blocks-per-file.receipts")]
    pub blocks_per_file_receipts: Option<u64>,

    /// Number of blocks per file for the transaction senders segment.
    #[arg(long = "static-files.blocks-per-file.transaction-senders")]
    pub blocks_per_file_transaction_senders: Option<u64>,

    /// Number of blocks per file for the account changesets segment.
    #[arg(long = "static-files.blocks-per-file.account-change-sets")]
    pub blocks_per_file_account_change_sets: Option<u64>,

    /// Store receipts in static files instead of the database.
    ///
    /// When enabled, receipts will be written to static files on disk instead of the database.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "static-files.receipts", default_value_t = default_static_file_flag(), action = clap::ArgAction::Set)]
    pub receipts: bool,

    /// Store transaction senders in static files instead of the database.
    ///
    /// When enabled, transaction senders will be written to static files on disk instead of the
    /// database.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "static-files.transaction-senders", default_value_t = default_static_file_flag(), action = clap::ArgAction::Set)]
    pub transaction_senders: bool,

    /// Store account changesets in static files.
    ///
    /// When enabled, account changesets will be written to static files on disk instead of the
    /// database.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "static-files.account-change-sets", default_value_t = default_static_file_flag(), action = clap::ArgAction::Set)]
    pub account_changesets: bool,
}

impl StaticFilesArgs {
    /// Merges the CLI arguments with an existing [`StaticFilesConfig`], giving priority to CLI
    /// args.
    ///
    /// If `minimal` is true, uses [`MINIMAL_BLOCKS_PER_FILE`] blocks per file as the default for
    /// headers, transactions, and receipts segments.
    pub fn merge_with_config(&self, config: StaticFilesConfig, minimal: bool) -> StaticFilesConfig {
        let minimal_blocks_per_file = minimal.then_some(MINIMAL_BLOCKS_PER_FILE);
        StaticFilesConfig {
            blocks_per_file: BlocksPerFileConfig {
                headers: self
                    .blocks_per_file_headers
                    .or(minimal_blocks_per_file)
                    .or(config.blocks_per_file.headers),
                transactions: self
                    .blocks_per_file_transactions
                    .or(minimal_blocks_per_file)
                    .or(config.blocks_per_file.transactions),
                receipts: self
                    .blocks_per_file_receipts
                    .or(minimal_blocks_per_file)
                    .or(config.blocks_per_file.receipts),
                transaction_senders: self
                    .blocks_per_file_transaction_senders
                    .or(config.blocks_per_file.transaction_senders),
                account_change_sets: self
                    .blocks_per_file_account_change_sets
                    .or(config.blocks_per_file.account_change_sets),
            },
        }
    }
}

impl Default for StaticFilesArgs {
    fn default() -> Self {
        Self {
            blocks_per_file_headers: None,
            blocks_per_file_transactions: None,
            blocks_per_file_receipts: None,
            blocks_per_file_transaction_senders: None,
            blocks_per_file_account_change_sets: None,
            receipts: default_static_file_flag(),
            transaction_senders: default_static_file_flag(),
            account_changesets: default_static_file_flag(),
        }
    }
}
