//! clap [Args](clap::Args) for static files configuration

use clap::Args;
use reth_config::config::{BlocksPerFileConfig, StaticFilesConfig};

/// Parameters for static files configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
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
    #[arg(long = "static-files.blocks-per-file.transaction_senders")]
    pub blocks_per_file_transaction_senders: Option<u64>,
}

impl StaticFilesArgs {
    /// Merges the CLI arguments with an existing [`StaticFilesConfig`], giving priority to CLI
    /// args.
    pub fn merge_with_config(&self, config: StaticFilesConfig) -> StaticFilesConfig {
        StaticFilesConfig {
            blocks_per_file: BlocksPerFileConfig {
                headers: self.blocks_per_file_headers.or(config.blocks_per_file.headers),
                transactions: self
                    .blocks_per_file_transactions
                    .or(config.blocks_per_file.transactions),
                receipts: self.blocks_per_file_receipts.or(config.blocks_per_file.receipts),
                transaction_senders: self
                    .blocks_per_file_transaction_senders
                    .or(config.blocks_per_file.transaction_senders),
            },
        }
    }
}
