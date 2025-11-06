//! clap [Args](clap::Args) for static files configuration

use clap::Args;
use reth_config::config::StaticFilesConfig;

/// Parameters for static files configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "Static Files")]
pub struct StaticFilesArgs {
    /// Number of blocks per file for the headers segment.
    ///
    /// Defaults to 500,000 blocks if not specified.
    #[arg(long = "static-files.blocks-per-file.headers")]
    pub blocks_per_file_headers: Option<u64>,

    /// Number of blocks per file for the transactions segment.
    ///
    /// Defaults to 500,000 blocks if not specified.
    #[arg(long = "static-files.blocks-per-file.transactions")]
    pub blocks_per_file_transactions: Option<u64>,

    /// Number of blocks per file for the receipts segment.
    ///
    /// Defaults to 500,000 blocks if not specified.
    #[arg(long = "static-files.blocks-per-file.receipts")]
    pub blocks_per_file_receipts: Option<u64>,
}

impl StaticFilesArgs {
    /// Converts the CLI arguments to a [`StaticFilesConfig`].
    pub const fn to_config(&self) -> StaticFilesConfig {
        StaticFilesConfig {
            headers_blocks_per_file: self.blocks_per_file_headers,
            transactions_blocks_per_file: self.blocks_per_file_transactions,
            receipts_blocks_per_file: self.blocks_per_file_receipts,
        }
    }

    /// Merges the CLI arguments with an existing [`StaticFilesConfig`], giving priority to CLI
    /// args.
    pub fn merge_with_config(&self, config: StaticFilesConfig) -> StaticFilesConfig {
        StaticFilesConfig {
            headers_blocks_per_file: self
                .blocks_per_file_headers
                .or(config.headers_blocks_per_file),
            transactions_blocks_per_file: self
                .blocks_per_file_transactions
                .or(config.transactions_blocks_per_file),
            receipts_blocks_per_file: self
                .blocks_per_file_receipts
                .or(config.receipts_blocks_per_file),
        }
    }
}
