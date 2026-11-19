use crate::{
    common::CliNodeTypes,
    db::get::{maybe_json_value_parser, table_key},
};
use alloy_primitives::map::foldhash::fast::FixedState;
use clap::Parser;
use reth_chainspec::{EthereumHardforks, OP_MAINNET_BEDROCK_BLOCK, OVM_HEADER_1_HASH};
use reth_db::{static_file::visit_static_file_headers, DatabaseEnv};
use reth_db_api::{
    cursor::DbCursorRO,
    database::Database,
    table::{Decode, Table},
    tables,
    transaction::DbTx,
    DatabaseError, RawKey, RawTable, TableViewer, Tables,
};
use reth_db_common::DbTool;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithDBAdapter};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::{ProviderNodeTypes, StaticFileProvider},
    DBProvider, ProviderError, StaticFileProviderFactory,
};
use reth_prune_types::PruneSegment;
use reth_static_file_types::{ChangesetOffset, StaticFileSegment};
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
                    tool.provider_factory.db_ref(),
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
    db: &impl Database,
    segment: StaticFileSegment,
    start_block: Option<u64>,
    end_block: Option<u64>,
    limit: Option<usize>,
) -> eyre::Result<(u64, usize)> {
    let mut ranges = Vec::new();
    visit_static_file_headers(static_file_provider.directory(), |kind, fixed, header, rows| {
        if kind == segment && (header.block_range().is_some() || rows != 0) {
            ranges.push((fixed, header.block_range()));
        }
    })?;
    eyre::ensure!(!ranges.is_empty(), "No static files found for segment: {segment}");

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
    ranges.sort_unstable_by_key(|(fixed, _)| fixed.start());
    for (fixed_block_range, block_range) in ranges {
        if reached_limit {
            break;
        }
        if fixed_block_range.start().max(start_block) > fixed_block_range.end().min(end_block) {
            continue;
        }
        let block_range = block_range.ok_or_else(|| {
            eyre::eyre!("Missing block range in nonempty {segment} static file {fixed_block_range}")
        })?;
        eyre::ensure!(
            !block_range.is_empty(),
            "Invalid block range {block_range} in {segment} static file"
        );
        eyre::ensure!(
            fixed_block_range.contains(block_range.start()) &&
                fixed_block_range.contains(block_range.end()),
            "Block range {block_range} outside {segment} static file {fixed_block_range}"
        );
        let first = block_range.start().max(start_block);
        let last = block_range.end().min(end_block);
        if first > last {
            continue;
        }

        let jar_provider = static_file_provider
            .get_segment_provider_for_range(segment, || Some(fixed_block_range), None)?
            .ok_or_else(|| {
                eyre::eyre!(
                    "Failed to get segment provider for segment {} at range {}",
                    segment,
                    block_range
                )
            })?;

        let header = jar_provider.user_header();
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
            let expected = if segment.is_headers() {
                block_range.len()
            } else if let Some(txs) = header.tx_range() {
                eyre::ensure!(
                    !txs.is_empty(),
                    "Invalid transaction range {txs} in {segment} static file {block_range}"
                );
                txs.len()
            } else {
                0
            };
            let actual = jar_provider.rows();
            eyre::ensure!(
                expected == u64::try_from(actual)?,
                "Row count mismatch in {segment} static file {block_range}: expected {expected}, got {actual}"
            );

            let mut rows = 0..expected;
            if segment.is_headers() {
                rows = (first - block_range.start())..(last - block_range.start() + 1);
            } else if let Some(txs) = header.tx_range() &&
                (first > block_range.start() || last < block_range.end())
            {
                let tx = db.tx()?;
                let start = block_range.start();
                // Migration can pad a post-Bedrock receipt file with pruned pre-Bedrock blocks.
                if segment.is_receipts() &&
                    start < OP_MAINNET_BEDROCK_BLOCK &&
                    tx.get::<tables::HeaderNumbers>(OVM_HEADER_1_HASH)?.is_some()
                {
                    let last_legacy = OP_MAINNET_BEDROCK_BLOCK - 1;
                    let checkpoint = tx.get::<tables::PruneCheckpoints>(PruneSegment::Receipts)?;
                    let padded = if let Some(cp) = checkpoint &&
                        cp.block_number.is_some_and(|block| {
                            (last_legacy..block_range.end()).contains(&block)
                        }) {
                        match transaction_in_block_prefix(&tx, txs.start(), last_legacy)? {
                            Some(legacy) => !legacy,
                            None => cp.tx_number.is_some_and(|pruned| pruned < txs.start()),
                        }
                    } else {
                        false
                    };
                    eyre::ensure!(padded,
                        "Cannot determine per-block receipt rows in legacy OVM static file {block_range}"
                    );
                }
                let before = match start.checked_sub(1) {
                    Some(block) => transaction_in_block_prefix(&tx, txs.start(), block)?,
                    None => Some(false),
                };
                let through_end = transaction_in_block_prefix(&tx, txs.end(), block_range.end())?;
                eyre::ensure!(
                    before != Some(true) && through_end != Some(false),
                    "{segment} transaction range {txs} does not fit blocks {block_range}"
                );
                // Imported receipts can have extra rows even when the query itself is indexed.
                if segment.is_receipts() && (before.is_none() || through_end.is_none()) {
                    let missing = if before.is_none() { start - 1 } else { block_range.end() };
                    return Err(ProviderError::BlockBodyIndicesNotFound(missing).into())
                }
                // An unproven file edge cannot stand in for the requested (possibly capped)
                // boundary.
                let through = |block, cap| {
                    transaction_rows_through(&tx, txs.start(), block, cap)?.ok_or_else(|| {
                        eyre::Report::from(ProviderError::BlockBodyIndicesNotFound(block))
                    })
                };
                if let Some(block) = first.checked_sub(1) &&
                    (first > start || before.is_none())
                {
                    rows.start = through(block, expected)?;
                }
                if last < block_range.end() || through_end.is_none() {
                    let remaining = u64::try_from(limit - checksummer.total)?;
                    let cap = rows.start.saturating_add(remaining).min(expected);
                    rows.end = through(last, cap)?;
                }
                eyre::ensure!(
                    rows.start <= rows.end,
                    "Decreasing transaction row offsets {rows:?} for blocks {first}..={last}"
                );
            }
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

/// Whether `tx_num` belongs to a block at or before `block`, if either index proves it.
fn transaction_in_block_prefix(
    tx: &impl DbTx,
    tx_num: u64,
    block: u64,
) -> eyre::Result<Option<bool>> {
    if let Some(rows) = transaction_rows_through(tx, tx_num, block, 1)? {
        return Ok(Some(rows == 1))
    }

    // Keys are the last transaction in each indexed block. Across a gap only use the side
    // proved by the nearest entry; a seek can skip over expired history.
    let mut cursor = tx.cursor_read::<tables::TransactionBlocks>()?;
    let next = cursor.seek(tx_num)?;
    if let Some((last_tx, owner)) = next &&
        (last_tx == tx_num || owner <= block)
    {
        return Ok(Some(owner <= block))
    }
    let previous = if next.is_some() { cursor.prev()? } else { cursor.last()? };
    Ok(previous.is_some_and(|(_, owner)| owner >= block).then_some(false))
}

/// Returns the capped row count through `block`, or `None` if missing indices leave it ambiguous.
fn transaction_rows_through(
    tx: &impl DbTx,
    first_tx: u64,
    block: u64,
    cap: u64,
) -> eyre::Result<Option<u64>> {
    let mut cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
    let (previous, next) = match cursor.seek(block)? {
        exact @ Some((number, _)) if number == block => (exact, cursor.next()?),
        next @ Some(_) => (cursor.prev()?, next),
        None => (cursor.last()?, None),
    };
    let lower = if let Some((number, body)) = previous {
        body.first_tx_num()
            .checked_add(body.tx_count())
            .ok_or_else(|| eyre::eyre!("Transaction index overflow at block {number}"))?
    } else {
        0
    };
    let offset = |n: u64| n.saturating_sub(first_tx).min(cap);
    if let Some((next, body)) = next {
        let upper = body.first_tx_num();
        if let Some((prev, _)) = previous &&
            prev.checked_add(1) == Some(next)
        {
            eyre::ensure!(
                lower == upper,
                "Conflicting transaction boundary between blocks {prev} and {next}: {lower} != {upper}"
            );
        }
        eyre::ensure!(
            lower <= upper,
            "Decreasing transaction indices {lower}..{upper} around block {block}"
        );
        if block.checked_add(1) == Some(next) {
            return Ok(Some(offset(upper)))
        }
    }
    let lower = offset(lower);
    let upper = next.map_or(cap, |(_, body)| offset(body.first_tx_num()));
    Ok((previous.is_some_and(|(number, _)| number == block) || lower == upper).then_some(lower))
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
    use reth_db::test_utils::create_test_rw_db;
    use reth_db_api::{
        models::{AccountBeforeTx, CompactU256, StoredBlockBodyIndices},
        table::Compress,
        transaction::DbTxMut,
    };
    use reth_node_api::NodeTypes;
    use reth_provider::{test_utils::MockNodeTypes, StaticFileProviderBuilder, StaticFileWriter};
    use reth_prune_types::PruneCheckpoint;
    use reth_static_file_types::{SegmentHeader, SegmentRangeInclusive};

    fn test_files(
        dir: &tempfile::TempDir,
        blocks_per_file: u64,
    ) -> StaticFileProvider<<MockNodeTypes as NodeTypes>::Primitives> {
        StaticFileProviderBuilder::read_write(dir.path())
            .with_blocks_per_file(blocks_per_file)
            .build()
            .unwrap()
    }

    #[test]
    fn checksum_headers_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 3);
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
            checksum_static_file(&files, &create_test_rw_db(), segment, Some(2), Some(3), None)
                .unwrap(),
            (expected.finish(), 2)
        );
    }

    #[test]
    fn checksum_transaction_rows_by_block() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 3);
        let bodies = create_test_rw_db();
        let tx = bodies.tx_mut().unwrap();
        let segment = StaticFileSegment::TransactionSenders;
        let mut writer = files.get_writer(0, segment).unwrap();
        let mut next_tx = 0;
        for (block, tx_count) in (0..).zip([0, 1, 2, 1, 0, 1, 0, 0, 0]) {
            let indices = StoredBlockBodyIndices { first_tx_num: next_tx, tx_count };
            tx.put::<tables::BlockBodyIndices>(block, indices).unwrap();
            writer.increment_block(block).unwrap();
            for tx in indices.tx_num_range() {
                writer.append_transaction_sender(tx, &Address::repeat_byte(tx as u8)).unwrap();
            }
            next_tx = indices.next_tx_num();
        }
        writer.commit().unwrap();
        drop(writer);
        tx.commit().unwrap();

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

        // The last nonempty sender file holds transactions 3 and 4 in blocks 3..=5.
        let reopened = test_files(&dir, 3);
        let mut writer = reopened.get_writer(3, segment).unwrap();
        writer.user_header_mut().set_tx_range(4, 5);
        writer.commit().unwrap();
        let err =
            checksum_static_file(&reopened, &bodies, segment, Some(5), Some(5), None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "TransactionSenders transaction range 4..=5 does not fit blocks 3..=5"
        );
        writer.user_header_mut().set_tx_range(3, 4);
        writer.commit().unwrap();
        drop(writer);

        for (block, first_tx_num, tx_count, error) in [
            (4, 3, 0, "Conflicting transaction boundary between blocks 3 and 4: 3 != 4"),
            (1, 3, 0, "Conflicting transaction boundary between blocks 0 and 1: 3 != 0"),
            (1, u64::MAX, 1, "Transaction index overflow at block 0"),
        ] {
            let tx = bodies.tx_mut().unwrap();
            tx.put::<tables::BlockBodyIndices>(
                block - 1,
                StoredBlockBodyIndices { first_tx_num, tx_count },
            )
            .unwrap();
            tx.commit().unwrap();
            let err =
                checksum_static_file(&files, &bodies, segment, Some(block), Some(block), None)
                    .unwrap_err();
            assert_eq!(err.to_string(), error);
        }

        let no_indices = create_test_rw_db();
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

    #[test]
    fn checksum_rejects_inconsistent_segment_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 3);
        let segment = StaticFileSegment::Headers;
        let mut writer = files.get_writer(0, segment).unwrap();
        let genesis = Header::default();
        writer.append_header(&genesis, &genesis.hash_slow()).unwrap();
        let extra = Header { number: 1, ..Default::default() };
        writer.append_header_direct(&extra, Default::default(), &extra.hash_slow()).unwrap();
        writer.commit().unwrap();
        drop(writer);

        let err = checksum_static_file(&files, &create_test_rw_db(), segment, None, None, None)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Row count mismatch in Headers static file 0..=0: expected 1, got 2"
        );

        let segment = StaticFileSegment::TransactionSenders;
        let mut writer = files.get_writer(0, segment).unwrap();
        writer.increment_block(0).unwrap();
        writer.append_transaction_sender(0, &Address::ZERO).unwrap();
        writer.commit().unwrap();
        drop(writer);

        for (block_start, tx_start, tx_end, error) in [
            (
                0,
                0,
                1,
                "Row count mismatch in TransactionSenders static file 0..=0: expected 2, got 1",
            ),
            (0, 1, 0, "Invalid transaction range 1..=0 in TransactionSenders static file 0..=0"),
            (1, 0, 0, "Invalid block range 1..=0 in TransactionSenders static file"),
        ] {
            let mut writer = files.get_writer(0, segment).unwrap();
            writer.user_header_mut().set_block_range(block_start, 0);
            writer.user_header_mut().set_tx_range(tx_start, tx_end);
            writer.commit().unwrap();
            drop(writer);
            let err =
                checksum_static_file(&files, &create_test_rw_db(), segment, None, None, Some(1))
                    .unwrap_err();
            assert_eq!(err.to_string(), error);
        }
    }

    #[test]
    fn checksum_validates_only_selected_files() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 2);
        let db = create_test_rw_db();
        let segment = StaticFileSegment::Headers;
        let mut writer = files.get_writer(0, segment).unwrap();
        for number in 0..=2 {
            let header = Header { number, ..Default::default() };
            writer.append_header(&header, &header.hash_slow()).unwrap();
        }
        writer.commit().unwrap();
        let expected = writer.user_header().expected_block_range();
        for (actual, error) in [
            (None, "Missing block range in nonempty Headers static file 2..=3"),
            (
                Some(SegmentRangeInclusive::new(3, 2)),
                "Invalid block range 3..=2 in Headers static file",
            ),
            (
                Some(SegmentRangeInclusive::new(1, 2)),
                "Block range 1..=2 outside Headers static file 2..=3",
            ),
        ] {
            *writer.user_header_mut() = SegmentHeader::new(expected, actual, None, segment);
            writer.commit().unwrap();
            assert_eq!(
                checksum_static_file(&files, &db, segment, None, Some(1), None).unwrap().1,
                2
            );
            assert_eq!(
                checksum_static_file(&files, &db, segment, None, None, None)
                    .unwrap_err()
                    .to_string(),
                error
            );
        }
    }

    #[test]
    fn checksum_receipts_require_transaction_numbers_within_file_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 3);
        let db = create_test_rw_db();
        let segment = StaticFileSegment::Receipts;
        let mut writer = files.get_writer(3, segment).unwrap();
        let tx = db.tx_mut().unwrap();
        for block in 3..6 {
            tx.put::<tables::BlockBodyIndices>(
                block,
                StoredBlockBodyIndices { first_tx_num: block, tx_count: 1 },
            )
            .unwrap();
            writer.increment_block(block).unwrap();
            // The receipt for block 3 was pruned.
            if block > 3 {
                writer.append_receipt(block, &Default::default()).unwrap();
            }
        }
        writer.commit().unwrap();
        tx.commit().unwrap();
        assert_eq!(
            checksum_static_file(&files, &db, segment, Some(4), Some(4), None).unwrap().1,
            1
        );
        for (start, end) in [(2, 3), (5, 6)] {
            writer.user_header_mut().set_tx_range(start, end);
            writer.commit().unwrap();
            assert_eq!(checksum_static_file(&files, &db, segment, None, None, None).unwrap().1, 2);
            let err =
                checksum_static_file(&files, &db, segment, Some(4), Some(4), Some(1)).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("Receipts transaction range {start}..={end} does not fit blocks 3..=5")
            );
        }

        writer.user_header_mut().set_tx_range(4, 5);
        writer.commit().unwrap();
        let tx = db.tx_mut().unwrap();
        tx.delete::<tables::BlockBodyIndices>(5, None).unwrap();
        tx.put::<tables::TransactionBlocks>(9, 8).unwrap();
        tx.commit().unwrap();
        let query = || checksum_static_file(&files, &db, segment, Some(4), Some(4), Some(1));
        assert!(matches!(
            query().unwrap_err().downcast_ref::<ProviderError>(),
            Some(ProviderError::BlockBodyIndicesNotFound(5))
        ));
        let tx = db.tx_mut().unwrap();
        tx.put::<tables::TransactionBlocks>(5, 5).unwrap();
        tx.commit().unwrap();
        assert_eq!(query().unwrap().1, 1);
    }

    #[test]
    fn checksum_imported_ovm_receipts() {
        for (first, pruned) in [
            (0, false),
            (OP_MAINNET_BEDROCK_BLOCK + 1, false),
            (OP_MAINNET_BEDROCK_BLOCK - 1, true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let files = test_files(&dir, 2);
            let db = create_test_rw_db();
            let tx = db.tx_mut().unwrap();
            tx.put::<tables::HeaderNumbers>(OVM_HEADER_1_HASH, 1).unwrap();
            let segment = StaticFileSegment::Receipts;
            let mut writer = files.get_writer(first, segment).unwrap();
            for tx_num in 0..2 {
                let block = first + tx_num;
                tx.put::<tables::BlockBodyIndices>(
                    block,
                    StoredBlockBodyIndices { first_tx_num: tx_num, tx_count: 1 },
                )
                .unwrap();
                writer.increment_block(block).unwrap();
                if !pruned || tx_num != 0 {
                    writer.append_receipt(tx_num, &Default::default()).unwrap();
                }
            }
            writer.commit().unwrap();
            drop(writer);
            tx.commit().unwrap();
            assert_eq!(
                checksum_static_file(&files, &db, segment, None, None, None).unwrap().1,
                if pruned { 1 } else { 2 }
            );
            let partial = || {
                checksum_static_file(&files, &db, segment, Some(first + 1), Some(first + 1), None)
            };
            if first < OP_MAINNET_BEDROCK_BLOCK {
                assert_eq!(
                    partial().unwrap_err().to_string(),
                    format!("Cannot determine per-block receipt rows in legacy OVM static file {first}..={}", first+1)
                );
            }
            if pruned {
                for tx_number in [Some(0), None] {
                    let tx = db.tx_mut().unwrap();
                    tx.put::<tables::PruneCheckpoints>(
                        PruneSegment::Receipts,
                        PruneCheckpoint { tx_number, ..PruneCheckpoint::pruned_through(first) },
                    )
                    .unwrap();
                    tx.commit().unwrap();
                    assert_eq!(partial().unwrap().1, 1);
                }
            }
            if first >= OP_MAINNET_BEDROCK_BLOCK {
                assert_eq!(partial().unwrap().1, 1);
            }
        }
    }

    #[test]
    fn checksum_sparse_transaction_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 10);
        let db = create_test_rw_db();
        let segment = StaticFileSegment::TransactionSenders;
        let mut writer = files.get_writer(0, segment).unwrap();
        for (tx_num, block) in (0u8..).zip([6, 10, 16]) {
            writer.ensure_at_block(block).unwrap();
            writer
                .append_transaction_sender(u64::from(tx_num), &Address::repeat_byte(tx_num))
                .unwrap();
        }
        writer.ensure_at_block(19).unwrap();
        writer.commit().unwrap();
        drop(writer);

        // Dummy blocks 1..=3 and nonempty blocks 6 and 16 have no body indices.
        let tx = db.tx_mut().unwrap();
        for (block, first_tx_num, tx_count) in
            [(0, 0, 0), (4, 0, 0), (7, 1, 0), (10, 1, 1), (14, 2, 0), (17, 3, 0)]
        {
            tx.put::<tables::BlockBodyIndices>(
                block,
                StoredBlockBodyIndices { first_tx_num, tx_count },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        for (start, end, limit, txs) in [
            (None, Some(2), None, 0..0),
            (Some(3), None, None, 0..3),
            (None, Some(6), None, 0..1),
            (None, Some(12), None, 0..2), // Equal bounds across a gap.
            (None, Some(15), Some(2), 0..2), // One row was already hashed in the first file.
            (Some(15), Some(18), Some(1), 2..3), // Limit relative to a nonzero file offset.
            (None, Some(18), None, 0..3), // Exhausted file with unindexed trailing blocks.
        ] {
            let mut expected = checksum_hasher();
            for tx in txs.clone() {
                expected.write(Address::repeat_byte(tx).as_slice());
            }
            assert_eq!(
                checksum_static_file(&files, &db, segment, start, end, limit).unwrap(),
                (expected.finish(), txs.len())
            );
        }
        for (end, limit) in [(5, None), (15, Some(3))] {
            let err =
                checksum_static_file(&files, &db, segment, None, Some(end), limit).unwrap_err();
            assert!(matches!(err.downcast_ref::<ProviderError>(),
                Some(ProviderError::BlockBodyIndicesNotFound(block)) if *block == end));
        }

        let tx = db.tx_mut().unwrap();
        tx.put::<tables::BlockBodyIndices>(
            14,
            StoredBlockBodyIndices { first_tx_num: 1, tx_count: 0 },
        )
        .unwrap();
        tx.commit().unwrap();
        for boundary in [10, 12, 13] {
            let err = checksum_static_file(&files, &db, segment, None, Some(boundary), Some(2))
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("Decreasing transaction indices 2..1 around block {boundary}")
            );
        }
    }

    #[test]
    fn checksum_changesets_ignore_transaction_range() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 3);
        let segment = StaticFileSegment::AccountChangeSets;
        let change = AccountBeforeTx { address: Address::repeat_byte(1), info: None };
        let mut expected = checksum_hasher();
        expected.write(&1u64.to_be_bytes());
        expected.write(change.address.as_slice());
        let mut writer = files.get_writer(0, segment).unwrap();
        writer.append_account_changeset(Vec::new(), 0).unwrap();
        writer.append_account_changeset(vec![change], 1).unwrap();
        writer.append_account_changeset(Vec::new(), 2).unwrap();
        writer.user_header_mut().set_tx_range(0, 0);
        writer.commit().unwrap();
        drop(writer);

        assert_eq!(
            checksum_static_file(&files, &create_test_rw_db(), segment, Some(1), Some(1), None)
                .unwrap(),
            (expected.finish(), 1)
        );
    }
}
