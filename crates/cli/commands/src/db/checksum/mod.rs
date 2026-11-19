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
    cursor::DbCursorRO,
    table::{Decode, Table},
    transaction::DbTx,
    DatabaseError, RawKey, RawTable, TableViewer, Tables,
};
use reth_db_common::DbTool;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithDBAdapter};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::{ProviderNodeTypes, StaticFileProvider},
    DBProvider, StaticFileProviderFactory,
};
use reth_static_file_types::{ChangesetOffset, StaticFileSegment};
use reth_storage_api::BlockBodyIndicesProvider;
use std::{
    hash::{BuildHasher, Hasher},
    ops::Bound,
    time::{Duration, Instant},
};
use tracing::{info, warn};

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
                let static_files = tool.provider_factory.static_file_provider();
                if let Err(err) = static_files.check_consistency(&tool.provider_factory.provider()?)
                {
                    warn!("Error checking consistency of static files: {err}");
                }
                checksum_static_file(
                    &static_files,
                    &tool.provider_factory,
                    segment,
                    start_block,
                    end_block,
                    limit,
                )?;
            }
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

fn checksum_static_file(
    static_file_provider: &StaticFileProvider<impl NodePrimitives>,
    indices: &impl BlockBodyIndicesProvider,
    segment: StaticFileSegment,
    start_block: Option<u64>,
    end_block: Option<u64>,
    limit: Option<usize>,
) -> eyre::Result<(u64, usize)> {
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

    info!(
        "Computing checksum for {} static files, start_block={}, end_block={}, limit={:?}",
        segment,
        start_block,
        end_block,
        if limit == usize::MAX { None } else { Some(limit) }
    );

    let mut reached_limit = limit == 0;
    for (block_range, header) in ranges.iter().sorted_by_key(|(range, _)| range.start()) {
        if reached_limit {
            break;
        }
        let first = block_range.start().max(start_block);
        let last = block_range.end().min(end_block);
        if first > last {
            continue;
        }

        let rows = if segment.is_headers() {
            (first - block_range.start())..(last - block_range.start() + 1)
        } else if let Some(txs) = header.tx_range() {
            // Whole files do not need body indices. Only resolve boundaries inside a file.
            let rows_through = |block| {
                indices
                    .next_tx_num_after_block(block)
                    .map(|next| next.saturating_sub(txs.start()).min(txs.len()))
            };
            let start = if first == block_range.start() { 0 } else { rows_through(first - 1)? };
            let end = if last == block_range.end() { txs.len() } else { rows_through(last)? };
            start..end
        } else {
            0..0
        };

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
            let input = ChangeBasedChecksumInput {
                segment,
                block_range_start: block_range.start(),
                start_block,
                end_block,
                offsets: &offsets,
            };

            reached_limit = checksum_change_based_segment(&mut checksummer, input, &mut cursor)?;
        } else {
            for index in rows {
                let row = cursor.row_by_number(usize::try_from(index)?)?.ok_or_else(|| {
                    eyre::eyre!("Missing row {index} in {segment} static file {block_range}")
                })?;
                if checksummer.write_row(&row) {
                    reached_limit = true;
                    break;
                }
            }
        }

        // Explicitly drop provider before removing from cache to avoid deadlock
        drop(jar_provider);
        static_file_provider.remove_cached_provider(segment, fixed_block_range.end());
    }

    let (checksum, total) = checksummer.finish();
    let elapsed = start_time.elapsed();

    info!(
        "Checksum for static file segment `{}`: {:#x} ({} entries, elapsed: {:?})",
        segment, checksum, total, elapsed
    );

    Ok((checksum, total))
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
        log_checksum_start(&self.start_key, &self.end_key, self.limit);

        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let parse_key = |key: &str| table_key::<T>(key).map(RawKey::new);
        let start = self.start_key.as_deref().map(parse_key).transpose()?;
        let end = self.end_key.as_deref().map(parse_key).transpose()?;
        let bound = |key: Option<RawKey<T::Key>>| key.map_or(Bound::Unbounded, Bound::Included);
        let mut entries = cursor
            .walk_range((bound(start), bound(end)))?
            .map(|entry| entry.map(|(k, v)| (k.into_key(), v.into_value())));

        checksum_entries(
            &mut entries,
            self.limit.unwrap_or(usize::MAX),
            &|key| Ok(serde_json::to_string(&T::Key::decode(key)?).unwrap_or_default()),
            T::NAME,
        )
    }
}

fn log_checksum_start(start: &Option<String>, end: &Option<String>, limit: Option<usize>) {
    info!("Start computing checksum, start={:?}, end={:?}, limit={:?}", start, end, limit);
}

/// Hashes the raw key-value entries of a table and logs the result.
///
/// `key_json` serializes a raw key to JSON, as it can only be decoded with the table's key type.
fn checksum_entries(
    entries: &mut dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>), DatabaseError>>,
    limit: usize,
    key_json: &dyn Fn(&[u8]) -> eyre::Result<String>,
    table: &str,
) -> eyre::Result<(u64, Duration)> {
    let start_time = Instant::now();
    let mut hasher = checksum_hasher();
    let mut total = 0;

    let mut enumerate_start_key = None;
    let mut enumerate_end_key = None;
    for (index, entry) in entries.enumerate() {
        let (k, v) = entry?;

        if index.is_multiple_of(PROGRESS_LOG_INTERVAL) {
            info!("Hashed {index} entries.");
        }

        hasher.write(&k);
        hasher.write(&v);

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
        info!("start-key: {}", key_json(&s)?);
        info!("end-key: {}", key_json(&e)?);
    }

    let checksum = hasher.finish();
    let elapsed = start_time.elapsed();

    info!("Checksum for table `{}`: {:#x} (elapsed: {:?})", table, checksum, elapsed);

    Ok((checksum, elapsed))
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

struct ChangeBasedChecksumInput<'a> {
    segment: StaticFileSegment,
    block_range_start: u64,
    start_block: u64,
    end_block: u64,
    offsets: &'a [ChangesetOffset],
}

fn checksum_change_based_segment<H: Hasher>(
    checksummer: &mut Checksummer<H>,
    input: ChangeBasedChecksumInput<'_>,
    cursor: &mut reth_db::static_file::StaticFileCursor<'_>,
) -> eyre::Result<bool> {
    let ChangeBasedChecksumInput { segment, block_range_start, start_block, end_block, offsets } =
        input;
    let is_storage = segment.is_storage_change_sets();
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::Address;
    use reth_db_api::{
        models::{CompactU256, StoredBlockBodyIndices},
        table::Compress,
    };
    use reth_node_api::NodeTypes;
    use reth_provider::{
        test_utils::{MockEthProvider, MockNodeTypes},
        ProviderError, StaticFileProviderBuilder, StaticFileWriter,
    };

    #[test]
    fn checksum_headers_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let files = StaticFileProviderBuilder::read_write(dir.path())
            .with_blocks_per_file(3)
            .build::<<MockNodeTypes as NodeTypes>::Primitives>()
            .unwrap();
        let segment = StaticFileSegment::Headers;
        let mut writer = files.get_writer(0, segment).unwrap();
        let mut expected = checksum_hasher();
        for number in 0..6 {
            let header = Header { number, ..Default::default() };
            let hash = header.hash_slow();
            writer.append_header(&header, &hash).unwrap();
            if (2..=3).contains(&number) {
                expected.write(&header.compress());
                expected.write(&CompactU256::default().compress());
                expected.write(hash.as_slice());
            }
        }
        writer.commit().unwrap();
        drop(writer);

        assert_eq!(
            checksum_static_file(
                &files,
                &MockEthProvider::default(),
                segment,
                Some(2),
                Some(3),
                None
            )
            .unwrap(),
            (expected.finish(), 2)
        );
    }

    #[test]
    fn checksum_transaction_rows_by_block() {
        let dir = tempfile::tempdir().unwrap();
        let files = StaticFileProviderBuilder::read_write(dir.path())
            .with_blocks_per_file(3)
            .build::<<MockNodeTypes as NodeTypes>::Primitives>()
            .unwrap();
        let bodies = MockEthProvider::default();
        let segment = StaticFileSegment::TransactionSenders;
        let mut writer = files.get_writer(0, segment).unwrap();
        let mut next_tx = 0;
        for (block, tx_count) in (0..).zip([0, 1, 2, 1, 0, 1, 0, 0, 0]) {
            let indices = StoredBlockBodyIndices { first_tx_num: next_tx, tx_count };
            bodies.add_block_body_indices(block, indices);
            writer.increment_block(block).unwrap();
            for tx in indices.tx_num_range() {
                writer.append_transaction_sender(tx, &Address::repeat_byte(tx as u8)).unwrap();
            }
            next_tx = indices.next_tx_num();
        }
        writer.commit().unwrap();
        drop(writer);

        for (start, end, limit, expected_txs) in [
            (Some(0), Some(0), None, 0..0),
            (Some(2), Some(2), None, 1..3),
            (Some(2), Some(3), None, 1..4),
            (Some(2), None, Some(3), 1..4),
            (Some(4), Some(4), None, 4..4),
            (Some(7), Some(7), None, 5..5),
            (None, Some(2), None, 0..3),
            (None, None, Some(0), 0..0),
            (Some(3), Some(1), None, 0..0),
        ] {
            let mut expected = checksum_hasher();
            for tx in expected_txs.clone() {
                expected.write(Address::repeat_byte(tx).as_slice());
            }
            assert_eq!(
                checksum_static_file(&files, &bodies, segment, start, end, limit).unwrap(),
                (expected.finish(), expected_txs.len()),
                "start={start:?} end={end:?} limit={limit:?}",
            );
        }

        let no_indices = MockEthProvider::default();
        assert_eq!(
            checksum_static_file(&files, &no_indices, segment, None, None, None).unwrap().1,
            5
        );
        let err =
            checksum_static_file(&files, &no_indices, segment, None, Some(1), None).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<ProviderError>(),
            Some(ProviderError::BlockBodyIndicesNotFound(1))
        ));
    }
}
