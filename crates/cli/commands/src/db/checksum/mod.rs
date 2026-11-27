use crate::{
    common::CliNodeTypes,
    db::get::{maybe_json_value_parser, table_key},
};
use alloy_primitives::map::foldhash::fast::FixedState;
use clap::Parser;
use reth_chainspec::EthereumHardforks;
use reth_db::{
    static_file::{iter_static_files, StaticFileEntry},
    DatabaseEnv,
};
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
use reth_static_file_types::{ChangesetOffset, SegmentRangeInclusive, StaticFileSegment};
use std::{
    hash::{BuildHasher, Hasher},
    ops::{Bound, Range},
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

fn checksum_static_file<N: NodePrimitives>(
    static_file_provider: &StaticFileProvider<N>,
    db: &impl Database,
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

    info!(
        "Computing checksum for {} static files, start_block={}, end_block={}, limit={:?}",
        segment,
        start_block,
        end_block,
        if limit == usize::MAX { None } else { Some(limit) }
    );

    let mut reached_limit = false;
    for StaticFileEntry { file_range, block_range, header } in ranges {
        if reached_limit || block_range.start() > end_block {
            break;
        }
        let selected = SegmentRangeInclusive::new(
            block_range.start().max(start_block),
            block_range.end().min(end_block),
        );
        if selected.is_empty() {
            continue;
        }

        let jar_provider = static_file_provider
            .get_segment_provider_for_range(segment, || Some(*file_range), None)?
            .ok_or_else(|| {
                eyre::eyre!(
                    "Failed to get segment provider for segment {} at range {}",
                    segment,
                    block_range
                )
            })?;
        let mut cursor = jar_provider.cursor()?;

        if segment.is_change_based() {
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
            let tx_range = header.tx_range().filter(|range| !range.is_empty());
            let rows = if segment.is_headers() {
                (selected.start() - block_range.start())..(selected.end() - block_range.start() + 1)
            } else if selected == *block_range {
                0..tx_range.map_or(0, |txs| txs.len())
            } else if let Some(txs) = tx_range {
                partial_transaction_rows(&db.tx()?, segment, *block_range, txs, selected)?
            } else {
                0..0
            };
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
        static_file_provider.remove_cached_provider(segment, file_range.end());
    }

    let (checksum, total) = checksummer.finish();
    let elapsed = start_time.elapsed();

    info!(
        "Checksum for static file segment `{}`: {:#x} ({} entries, elapsed: {:?})",
        segment, checksum, total, elapsed
    );

    Ok((checksum, total))
}

/// Maps the requested blocks to offsets in a transaction-numbered jar.
fn partial_transaction_rows(
    tx: &impl DbTx,
    segment: StaticFileSegment,
    blocks: SegmentRangeInclusive,
    txs: SegmentRangeInclusive,
    selected: SegmentRangeInclusive,
) -> eyre::Result<Range<u64>> {
    let mut cursor = tx.cursor_read::<tables::BlockBodyIndices>()?;
    let mut body = |block| {
        match cursor.seek(block)? {
            Some((indexed, indices)) if indexed == block => Ok(indices),
            // Snap sync and dummy chains can omit indices before the first stored transaction.
            Some((_, indices)) if indices.first_tx_num() == 0 => Ok(Default::default()),
            _ => Err(ProviderError::BlockBodyIndicesNotFound(block)),
        }
    };
    if segment.is_receipts() {
        // Padded pre-Bedrock OVM receipt files can extend past their blocks' transaction bounds.
        let first_tx = body(blocks.start())?.first_tx_num();
        let end_fits = match body(blocks.end()) {
            Ok(last) => txs.end() < last.next_tx_num(),
            // Static files can be ahead of the database after an interrupted write.
            Err(ProviderError::BlockBodyIndicesNotFound(block)) if selected.end() < block => true,
            Err(err) => return Err(err.into()),
        };
        eyre::ensure!(
            first_tx <= txs.start() && end_fits,
            "{segment} transaction range {txs} does not fit blocks {blocks}"
        );
    }
    let offset = |n: u64| n.saturating_sub(txs.start()).min(txs.len());
    let from = if selected.start() == blocks.start() {
        0
    } else {
        offset(body(selected.start())?.first_tx_num())
    };
    let to = if selected.end() == blocks.end() {
        txs.len()
    } else {
        offset(body(selected.end())?.next_tx_num())
    };
    eyre::ensure!(from <= to, "Decreasing transaction offsets for blocks {selected}");
    Ok(from..to)
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
    use alloy_consensus::{Header, TxType};
    use alloy_primitives::Address;
    use reth_db::test_utils::create_test_rw_db;
    use reth_db_api::{
        models::{CompactU256, StoredBlockBodyIndices},
        table::Compress,
        transaction::DbTxMut,
    };
    use reth_ethereum_primitives::Receipt;
    use reth_node_api::NodeTypes;
    use reth_provider::{test_utils::MockNodeTypes, StaticFileProviderBuilder, StaticFileWriter};

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
        // Block 1's own index determines its end even when the following index is missing.
        bodies.update(|tx| tx.delete::<tables::BlockBodyIndices>(2, None).unwrap()).unwrap();
        assert_eq!(
            checksum_static_file(&files, &bodies, segment, None, Some(1), None).unwrap().1,
            1
        );
        let err =
            checksum_static_file(&files, &bodies, segment, Some(2), Some(2), None).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<ProviderError>(),
            Some(ProviderError::BlockBodyIndicesNotFound(2))
        ));
    }

    #[test]
    fn checksum_receipts_require_transaction_numbers_within_file_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 3);
        let db = create_test_rw_db();
        let segment = StaticFileSegment::Receipts;
        let receipt = |block| Receipt::standard(TxType::Legacy, false, block, Vec::new());
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
                writer.append_receipt(block, &receipt(block)).unwrap();
            }
        }
        writer.commit().unwrap();
        tx.commit().unwrap();
        let block = |n| checksum_static_file(&files, &db, segment, Some(n), Some(n), None);
        assert_eq!(block(3).unwrap(), (checksum_hasher().finish(), 0));
        let mut expected = checksum_hasher();
        expected.write(&receipt(4).compress());
        assert_eq!(block(4).unwrap(), (expected.finish(), 1));
        for (start, end) in [(2, 3), (5, 6)] {
            writer.user_header_mut().set_tx_range(start, end);
            writer.commit().unwrap();
            assert_eq!(
                block(4).unwrap_err().to_string(),
                format!("Receipts transaction range {start}..={end} does not fit blocks 3..=5")
            );
        }
        assert_eq!(checksum_static_file(&files, &db, segment, None, None, None).unwrap().1, 2);
    }

    #[test]
    fn checksum_receipts_with_unindexed_prefix_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let files = test_files(&dir, 10);
        let db = create_test_rw_db();
        let segment = StaticFileSegment::Receipts;
        let mut writer = files.get_writer(3, segment).unwrap();
        writer.initialize_pruned_anchor(3).unwrap();
        writer.increment_block(4).unwrap();
        writer.increment_block(5).unwrap();
        let receipt = Receipt::standard(TxType::Legacy, false, 42, Vec::new());
        writer.append_receipt(0, &receipt).unwrap();
        writer.increment_block(6).unwrap();
        writer.append_receipt(1, &Default::default()).unwrap();
        writer.commit().unwrap();
        drop(writer);
        db.update(|tx| {
            tx.put::<tables::BlockBodyIndices>(
                5,
                StoredBlockBodyIndices { first_tx_num: 0, tx_count: 1 },
            )
        })
        .unwrap()
        .unwrap();

        let checksum =
            |start, end| checksum_static_file(&files, &db, segment, Some(start), Some(end), None);
        let mut expected = checksum_hasher();
        expected.write(&receipt.compress());
        assert_eq!(checksum(5, 5).unwrap(), (expected.finish(), 1));
        assert_eq!(checksum(4, 4).unwrap(), (checksum_hasher().finish(), 0));
        assert_eq!(checksum(4, 5).unwrap(), (expected.finish(), 1));
        assert!(matches!(
            checksum(5, 6).unwrap_err().downcast_ref::<ProviderError>(),
            Some(ProviderError::BlockBodyIndicesNotFound(6))
        ));
    }
}
