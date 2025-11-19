//! clap [Args](clap::Args) for static files configuration

use clap::Args;
use reth_config::config::{BlocksPerFileConfig, StaticFilesConfig};
use reth_provider::StorageSettings;

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

    /// Store receipts in static files instead of the database.
    ///
    /// When enabled, receipts will be written to static files on disk instead of the database.
    ///
    /// Note: This setting can only be configured at genesis initialization. Once
    /// the node has been initialized, changing this flag requires re-syncing from scratch.
    #[arg(long = "static-files.receipts")]
    pub receipts: bool,
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
            },
        }
    }

    /// Converts the static files arguments into [`StorageSettings`].
    pub const fn to_settings(&self) -> StorageSettings {
        if self.receipts {
            StorageSettings::new().with_receipts_in_static_files()
        } else {
            StorageSettings::legacy()
        }
    }
}
