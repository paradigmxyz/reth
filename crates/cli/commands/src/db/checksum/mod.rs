use crate::{
    common::CliNodeTypes,
    db::get::{maybe_json_value_parser, table_key},
};
use alloy_primitives::map::foldhash::fast::FixedState;
use clap::Parser;
use itertools::Itertools;
use reth_chainspec::EthereumHardforks;
use reth_db::{static_file::iter_static_files, DatabaseEnv};
use reth_db_api::{
    cursor::DbCursorRO, table::Table, transaction::DbTx, RawKey, RawTable, RawValue, TableViewer,
    Tables,
};
use reth_db_common::DbTool;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithDBAdapter};
use reth_provider::{providers::ProviderNodeTypes, DBProvider, StaticFileProviderFactory};
use reth_static_file_types::StaticFileSegment;
use std::{
    hash::{BuildHasher, Hasher},
    time::{Duration, Instant},
};
use tracing::{info, warn};

#[cfg(all(unix, feature = "rocksdb"))]
mod rocksdb;

/// Interval for logging progress during checksum computation.
const PROGRESS_LOG_INTERVAL: usize = 100_000;

#[derive(Parser, Debug)]
/// The arguments for the `reth db checksum` command
pub struct Command {
    #[command(subcommand)]
    subcommand: Subcommand,
}

#[derive(clap::Subcommand, Debug)]
enum Subcommand {
    /// Calculates the checksum of a database table
    Mdbx {
        /// The table name
        table: Tables,

        /// The start of the range to checksum.
        #[arg(long, value_parser = maybe_json_value_parser)]
        start_key: Option<String>,

        /// The end of the range to checksum.
        #[arg(long, value_parser = maybe_json_value_parser)]
        end_key: Option<String>,

        /// The maximum number of records that are queried and used to compute the
        /// checksum.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Calculates the checksum of a static file segment
    StaticFile {
        /// The static file segment
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
    },
    /// Calculates the checksum of a RocksDB table
    #[cfg(all(unix, feature = "rocksdb"))]
    Rocksdb {
        /// The RocksDB table
        #[arg(value_enum)]
        table: rocksdb::RocksDbTable,

        /// The maximum number of records to checksum.
        #[arg(long)]
        limit: Option<usize>,
    },
}

impl Command {
    /// Execute `db checksum` command
    pub fn execute<N: CliNodeTypes<ChainSpec: EthereumHardforks>>(
        self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, DatabaseEnv>>,
    ) -> eyre::Result<()> {
        warn!("This command should be run without the node running!");

        match self.subcommand {
            Subcommand::Mdbx { table, start_key, end_key, limit } => {
                table.view(&ChecksumViewer { tool, start_key, end_key, limit })?;
            }
            Subcommand::StaticFile { segment, start_block, end_block, limit } => {
                checksum_static_file(tool, segment, start_block, end_block, limit)?;
            }
            #[cfg(all(unix, feature = "rocksdb"))]
            Subcommand::Rocksdb { table, limit } => {
                rocksdb::checksum_rocksdb(tool, table, limit)?;
            }
        }

        Ok(())
    }
}

/// Creates a new hasher with the standard seed used for checksum computation.
fn checksum_hasher() -> impl Hasher {
    FixedState::with_seed(u64::from_be_bytes(*b"RETHRETH")).build_hasher()
}

fn checksum_static_file<N: CliNodeTypes<ChainSpec: EthereumHardforks>>(
    tool: &DbTool<NodeTypesWithDBAdapter<N, DatabaseEnv>>,
    segment: StaticFileSegment,
    start_block: Option<u64>,
    end_block: Option<u64>,
    limit: Option<usize>,
) -> eyre::Result<()> {
    let static_file_provider = tool.provider_factory.static_file_provider();
    if let Err(err) = static_file_provider.check_consistency(&tool.provider_factory.provider()?) {
        warn!("Error checking consistency of static files: {err}");
    }

    let static_files = iter_static_files(static_file_provider.directory())?;

    let ranges = static_files
        .get(segment)
        .ok_or_else(|| eyre::eyre!("No static files found for segment: {}", segment))?;

    let start_time = Instant::now();
    let mut hasher = checksum_hasher();
    let mut total = 0usize;
    let limit = limit.unwrap_or(usize::MAX);

    let start_block = start_block.unwrap_or(0);
    let end_block = end_block.unwrap_or(u64::MAX);

    info!(
        "Computing checksum for {} static files, start_block={}, end_block={}, limit={:?}",
        segment,
        start_block,
        end_block,
        if limit == usize::MAX { None } else { Some(limit) }
    );

    'outer: for (block_range, _header) in ranges.iter().sorted_by_key(|(range, _)| range.start()) {
        if block_range.end() < start_block || block_range.start() > end_block {
            continue;
        }

        let fixed_block_range = static_file_provider.find_fixed_range(segment, block_range.start());
        let jar_provider = static_file_provider
            .get_segment_provider_for_range(segment, || Some(fixed_block_range), None)?
            .ok_or_else(|| {
                eyre::eyre!(
                    "Failed to get segment provider for segment {} at range {}",
                    segment,
                    block_range
                )
            })?;

        let mut cursor = jar_provider.cursor()?;

        while let Ok(Some(row)) = cursor.next_row() {
            for col_data in row.iter() {
                hasher.write(col_data);
            }

            total += 1;

            if total.is_multiple_of(PROGRESS_LOG_INTERVAL) {
                info!("Hashed {total} entries.");
            }

            if total >= limit {
                break 'outer;
            }
        }

        // Explicitly drop provider before removing from cache to avoid deadlock
        drop(jar_provider);
        static_file_provider.remove_cached_provider(segment, fixed_block_range.end());
    }

    let checksum = hasher.finish();
    let elapsed = start_time.elapsed();

    info!(
        "Checksum for static file segment `{}`: {:#x} ({} entries, elapsed: {:?})",
        segment, checksum, total, elapsed
    );

    Ok(())
}

pub(crate) struct ChecksumViewer<'a, N: NodeTypesWithDB> {
    tool: &'a DbTool<N>,
    start_key: Option<String>,
    end_key: Option<String>,
    limit: Option<usize>,
}

impl<N: NodeTypesWithDB> ChecksumViewer<'_, N> {
    pub(crate) const fn new(tool: &'_ DbTool<N>) -> ChecksumViewer<'_, N> {
        ChecksumViewer { tool, start_key: None, end_key: None, limit: None }
    }
}

impl<N: ProviderNodeTypes> TableViewer<(u64, Duration)> for ChecksumViewer<'_, N> {
    type Error = eyre::Report;

    fn view<T: Table>(&self) -> Result<(u64, Duration), Self::Error> {
        let provider =
            self.tool.provider_factory.provider()?.disable_long_read_transaction_safety();
        let tx = provider.tx_ref();
        info!(
            "Start computing checksum, start={:?}, end={:?}, limit={:?}",
            self.start_key, self.end_key, self.limit
        );

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let walker = match (self.start_key.as_deref(), self.end_key.as_deref()) {
            (Some(start), Some(end)) => {
                let start_key = table_key::<T>(start).map(RawKey::new)?;
                let end_key = table_key::<T>(end).map(RawKey::new)?;
                cursor.walk_range(start_key..=end_key)?
            }
            (None, Some(end)) => {
                let end_key = table_key::<T>(end).map(RawKey::new)?;

                cursor.walk_range(..=end_key)?
            }
            (Some(start), None) => {
                let start_key = table_key::<T>(start).map(RawKey::new)?;
                cursor.walk_range(start_key..)?
            }
            (None, None) => cursor.walk_range(..)?,
        };

        let start_time = Instant::now();
        let mut hasher = checksum_hasher();
        let mut total = 0;

        let limit = self.limit.unwrap_or(usize::MAX);
        let mut enumerate_start_key = None;
        let mut enumerate_end_key = None;
        for (index, entry) in walker.enumerate() {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = entry?;

            if index.is_multiple_of(PROGRESS_LOG_INTERVAL) {
                info!("Hashed {index} entries.");
            }

            hasher.write(k.raw_key());
            hasher.write(v.raw_value());

            if enumerate_start_key.is_none() {
                enumerate_start_key = Some(k.clone());
            }
            enumerate_end_key = Some(k);

            total = index + 1;
            if total >= limit {
                break
            }
        }

        info!("Hashed {total} entries.");
        if let (Some(s), Some(e)) = (enumerate_start_key, enumerate_end_key) {
            info!("start-key: {}", serde_json::to_string(&s.key()?).unwrap_or_default());
            info!("end-key: {}", serde_json::to_string(&e.key()?).unwrap_or_default());
        }

        let checksum = hasher.finish();
        let elapsed = start_time.elapsed();

        info!("Checksum for table `{}`: {:#x} (elapsed: {:?})", T::NAME, checksum, elapsed);

        Ok((checksum, elapsed))
    }
}
