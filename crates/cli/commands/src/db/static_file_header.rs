use clap::{Parser, Subcommand};
use reth_cli::chainspec::ChainSpecParser;
use reth_provider::StaticFileProviderFactory;
use reth_static_file_types::StaticFileSegment;
use std::path::PathBuf;
use tracing::warn;

use crate::common::{AccessRights, CliNodeTypes, EnvironmentArgs};

/// The arguments for the `reth db static-file-header` command
#[derive(Parser, Debug)]
pub struct Command {
    #[command(subcommand)]
    source: Source,
}

/// Source for locating the static file
#[derive(Subcommand, Debug)]
enum Source {
    /// Query by segment and block number
    Block {
        /// Static file segment
        #[arg(value_enum)]
        segment: StaticFileSegment,
        /// Block number to query
        block: u64,
    },
    /// Query by path to static file
    Path {
        /// Path to the static file
        path: PathBuf,
    },
}

impl Command {
    /// Execute `db static-file-header` command
    pub fn execute<N: CliNodeTypes, C: ChainSpecParser<ChainSpec = N::ChainSpec>>(
        self,
        env: EnvironmentArgs<C>,
    ) -> eyre::Result<()> {
        // Try to initialize the environment as read-only. If it fails, try to initialize it as
        // read-only without consistency checks.
        let provider_factory = match env.init::<N>(AccessRights::RO) {
            Ok(env) => env,
            Err(err) => {
                warn!(?err, "Failed to initialize environment");
                env.init::<N>(AccessRights::RoInconsistent)?
            }
        }
        .provider_factory;
        let static_file_provider = provider_factory.static_file_provider();

        // Get the provider based on the source
        let provider = match self.source {
            Source::Path { path } => {
                static_file_provider.get_segment_provider_for_path(&path)?.ok_or_else(|| {
                    eyre::eyre!("Could not find static file segment for path: {}", path.display())
                })?
            }
            Source::Block { segment, block } => {
                static_file_provider.get_segment_provider(segment, block)?
            }
        };

        let header = provider.user_header();

        println!("Static File Segment Header");
        println!("==========================");
        println!("Segment: {}", header.segment());
        println!("Expected Block Range: {}", header.expected_block_range());
        println!("Block Range: {:?}", header.block_range());
        println!("Transaction Range: {:?}", header.tx_range());

        Ok(())
    }
}
