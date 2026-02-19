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
use reth_static_file_types::{ChangesetOffset, StaticFileSegment};
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

/// Accumulates a checksum over key-value entries, tracking count and limit.
struct Checksummer<H> {
    hasher: H,
    total: usize,
    limit: usize,
}

impl<H: Hasher> Checksummer<H> {
    fn new(hasher: H, limit: usize) -> Self {
        Self { hasher, total: 0, limit }
    }

    /// Hash a row's columns (non-changeset segments). Returns `true` if the limit is reached.
    fn write_row(&mut self, row: &[&[u8]]) -> bool {
        for col in row {
            self.hasher.write(col);
        }
        self.advance()
    }

    /// Hash a key + value as two separate writes, matching MDBX raw entry semantics.
    /// Write boundaries matter: foldhash rotates its accumulator by `len` on each `write`.
    fn write_entry(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.hasher.write(key);
        self.hasher.write(value);
        self.advance()
    }

    fn advance(&mut self) -> bool {
        self.total += 1;
        if self.total.is_multiple_of(PROGRESS_LOG_INTERVAL) {
            info!("Hashed {} entries.", self.total);
        }
        self.total >= self.limit
    }

    fn finish(self) -> (u64, usize) {
        (self.hasher.finish(), self.total)
    }
}

/// Reconstruct MDBX `StorageChangeSets` key/value boundaries from a static-file row.
///
/// MDBX layout:
/// - key: `BlockNumberAddress` => `[8B block_number][20B address]`
/// - value: `StorageEntry` => `[32B storage_key][compact U256 value]`
///
/// Static-file row layout for `StorageBeforeTx`:
/// - `[20B address][32B storage_key][compact U256 value]`
fn split_storage_changeset_row(block_number: u64, row: &[u8]) -> eyre::Result<([u8; 28], &[u8])> {
    if row.len() < 20 {
        return Err(eyre::eyre!(
            "Storage changeset row too short: expected at least 20 bytes, got {}",
            row.len()
        ));
    }

    let mut key_buf = [0u8; 28];
    key_buf[..8].copy_from_slice(&block_number.to_be_bytes());
    key_buf[8..].copy_from_slice(&row[..20]);
    Ok((key_buf, &row[20..]))
}

fn checksum_change_based_segment<H: Hasher>(
    checksummer: &mut Checksummer<H>,
    segment: StaticFileSegment,
    block_range_start: u64,
    start_block: u64,
    end_block: u64,
    is_storage: bool,
    offsets: &[ChangesetOffset],
    cursor: &mut reth_db::static_file::StaticFileCursor<'_>,
) -> eyre::Result<bool> {
    let mut reached_limit = false;

    for (offset_index, offset) in offsets.iter().enumerate() {
        let block_number = block_range_start + offset_index as u64;
        let include = block_number >= start_block && block_number <= end_block;

        for _ in 0..offset.num_changes() {
            let row = cursor.next_row()?.ok_or_else(|| {
                eyre::eyre!(
                    "Unexpected EOF while checksumming {} static file at range starting {}",
                    segment,
                    block_range_start
                )
            })?;

            if !include {
                continue;
            }

            // Reconstruct MDBX key/value write boundaries. foldhash rotates
            // its accumulator by `len` on each write(), so boundaries must
            // match exactly.
            let done = if is_storage {
                // StorageChangeSets: MDBX key = BlockNumberAddress (28B),
                // value = compact StorageEntry. Column 0 is compact
                // StorageBeforeTx = [20B address][32B key][compact U256].
                let col = row[0];
                let (key, value) = split_storage_changeset_row(block_number, col)?;
                checksummer.write_entry(&key, value)
            } else {
                // AccountChangeSets: MDBX key = BlockNumber (8B),
                // value = compact AccountBeforeTx (= column 0).
                checksummer.write_entry(&block_number.to_be_bytes(), row[0])
            };

            if done {
                reached_limit = true;
                break;
            }
        }

        if reached_limit {
            break;
        }
    }

    if !reached_limit && cursor.next_row()?.is_some() {
        return Err(eyre::eyre!(
            "Changeset offsets do not cover all rows for {} at range starting {}",
            segment,
            block_range_start
        ));
    }

    Ok(reached_limit)
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
    let limit = limit.unwrap_or(usize::MAX);
    let mut checksummer = Checksummer::new(checksum_hasher(), limit);

    let start_block = start_block.unwrap_or(0);
    let end_block = end_block.unwrap_or(u64::MAX);
    let is_change_based = segment.is_change_based();
    let is_storage = segment.is_storage_change_sets();

    info!(
        "Computing checksum for {} static files, start_block={}, end_block={}, limit={:?}",
        segment,
        start_block,
        end_block,
        if limit == usize::MAX { None } else { Some(limit) }
    );

    let mut reached_limit = false;
    for (block_range, _header) in ranges.iter().sorted_by_key(|(range, _)| range.start()) {
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

        if is_change_based {
            let offsets = jar_provider.read_changeset_offsets()?.ok_or_else(|| {
                eyre::eyre!(
                    "Missing changeset offsets sidecar for segment {} at range {}",
                    segment,
                    block_range
                )
            })?;

            reached_limit = checksum_change_based_segment(
                &mut checksummer,
                segment,
                block_range.start(),
                start_block,
                end_block,
                is_storage,
                &offsets,
                &mut cursor,
            )?;
        } else {
            while let Some(row) = cursor.next_row()? {
                if checksummer.write_row(&row) {
                    reached_limit = true;
                    break;
                }
            }
        }

        // Explicitly drop provider before removing from cache to avoid deadlock
        drop(jar_provider);
        static_file_provider.remove_cached_provider(segment, fixed_block_range.end());

        if reached_limit {
            break;
        }
    }

    let (checksum, total) = checksummer.finish();
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
                break;
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

#[cfg(test)]
mod tests {
    use super::split_storage_changeset_row;
    use alloy_primitives::Address;

    #[test]
    fn split_storage_changeset_row_ok() {
        let block = 0x0102_0304_0506_0708u64;
        let address = Address::repeat_byte(0x11);
        let mut row = Vec::new();
        row.extend_from_slice(address.as_slice());
        row.extend_from_slice(&[0x22; 32]); // storage key
        row.extend_from_slice(&[0x33, 0x44]); // compact U256 payload (test bytes)

        let (key, value) = split_storage_changeset_row(block, &row).expect("valid row");

        assert_eq!(&key[..8], &block.to_be_bytes());
        assert_eq!(&key[8..], address.as_slice());
        assert_eq!(value.len(), 34);
        assert_eq!(&value[..32], &[0x22; 32]);
        assert_eq!(&value[32..], &[0x33, 0x44]);
    }

    #[test]
    fn split_storage_changeset_row_too_short() {
        let err = split_storage_changeset_row(1, &[0xAA; 19]).expect_err("must fail");
        assert!(err.to_string().contains("too short"));
    }
}
