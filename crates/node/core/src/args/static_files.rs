//! clap [Args](clap::Args) for static files configuration

use clap::Args;
use reth_config::config::{BlocksPerFileConfig, StaticFilesConfig};

/// Blocks per static file when running in `--minimal` node.
///
/// 10000 blocks per static file allows us to prune all history every 10k blocks.
pub const MINIMAL_BLOCKS_PER_FILE: u64 = 10000;

/// Parameters for static files configuration
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy, Default)]
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

    /// Number of blocks per file for the storage changesets segment.
    #[arg(long = "static-files.blocks-per-file.storage-change-sets")]
    pub blocks_per_file_storage_change_sets: Option<u64>,
}

impl StaticFilesArgs {
    /// Merges the CLI arguments with an existing [`StaticFilesConfig`], giving priority to CLI
    /// args.
    ///
    /// If `minimal` is true, uses [`MINIMAL_BLOCKS_PER_FILE`] blocks per file as the default for
    /// all segments.
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
                    .or(minimal_blocks_per_file)
                    .or(config.blocks_per_file.transaction_senders),
                account_change_sets: self
                    .blocks_per_file_account_change_sets
                    .or(minimal_blocks_per_file)
                    .or(config.blocks_per_file.account_change_sets),
                storage_change_sets: self
                    .blocks_per_file_storage_change_sets
                    .or(minimal_blocks_per_file)
                    .or(config.blocks_per_file.storage_change_sets),
            },
        }
    }
}
