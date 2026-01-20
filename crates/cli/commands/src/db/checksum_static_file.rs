use crate::common::CliNodeTypes;
use alloy_primitives::map::foldhash::fast::FixedState;
use clap::Parser;
use itertools::Itertools;
use reth_chainspec::EthereumHardforks;
use reth_db::{static_file::iter_static_files, DatabaseEnv};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_provider::{DBProvider, StaticFileProviderFactory};
use reth_static_file_types::StaticFileSegment;
use std::{
    hash::{BuildHasher, Hasher},
    sync::Arc,
    time::Instant,
};
use tracing::{info, warn};

#[derive(Parser, Debug)]
/// The arguments for the `reth db checksum-static-file` command
pub struct Command {
    /// The static file segment to checksum.
    #[arg(value_enum)]
    segment: StaticFileSegment,

    /// The block number to start from (inclusive).
    #[arg(long)]
    start_block: Option<u64>,

    /// The block number to end at (inclusive).
    #[arg(long)]
    end_block: Option<u64>,

    /// The maximum number of rows to checksum.
    #[arg(long)]
    limit: Option<usize>,
}

impl Command {
    /// Execute `db checksum-static-file` command
    pub fn execute<N: CliNodeTypes<ChainSpec: EthereumHardforks>>(
        self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<()> {
        warn!("This command should be run without the node running!");

        let static_file_provider = tool.provider_factory.static_file_provider();
        if let Err(err) = static_file_provider.check_consistency(&tool.provider_factory.provider()?)
        {
            warn!("Error checking consistency of static files: {err}");
        }

        let static_files = iter_static_files(static_file_provider.directory())?;

        let ranges = static_files
            .get(&self.segment)
            .ok_or_else(|| eyre::eyre!("No static files found for segment: {}", self.segment))?;

        let start_time = Instant::now();
        let mut hasher = FixedState::with_seed(u64::from_be_bytes(*b"RETHRETH")).build_hasher();
        let mut total_rows = 0usize;
        let limit = self.limit.unwrap_or(usize::MAX);

        let start_block = self.start_block.unwrap_or(0);
        let end_block = self.end_block.unwrap_or(u64::MAX);

        info!(
            "Computing checksum for {} static files, start_block={}, end_block={}, limit={:?}",
            self.segment, start_block, end_block, self.limit
        );

        for (block_range, _header) in ranges.iter().sorted_by_key(|(range, _)| range.start()) {
            if block_range.end() < start_block || block_range.start() > end_block {
                continue;
            }

            let fixed_block_range =
                static_file_provider.find_fixed_range(self.segment, block_range.start());
            let jar_provider = static_file_provider
                .get_segment_provider_for_range(self.segment, || Some(fixed_block_range), None)?
                .ok_or_else(|| {
                    eyre::eyre!(
                        "Failed to get segment provider for segment {} at range {}",
                        self.segment,
                        block_range
                    )
                })?;

            let mut cursor = jar_provider.cursor()?;

            while let Ok(Some(row)) = cursor.next_row() {
                for col_data in row.iter() {
                    hasher.write(col_data);
                }

                total_rows += 1;

                if total_rows.is_multiple_of(100_000) {
                    info!("Hashed {total_rows} rows.");
                }

                if total_rows >= limit {
                    break;
                }
            }

            drop(jar_provider);
            static_file_provider.remove_cached_provider(self.segment, fixed_block_range.end());

            if total_rows >= limit {
                break;
            }
        }

        let checksum = hasher.finish();
        let elapsed = start_time.elapsed();

        info!(
            "Checksum for static file segment `{}`: {:#x} ({} rows, elapsed: {:?})",
            self.segment, checksum, total_rows, elapsed
        );

        Ok(())
    }
}
