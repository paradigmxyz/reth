//! Snapshot segment implementations and utilities.

mod transactions;
pub use transactions::Transactions;

mod headers;
pub use headers::Headers;

mod receipts;
pub use receipts::Receipts;

use reth_db::{
    cursor::DbCursorRO, database::Database, table::Table, transaction::DbTx, RawKey, RawTable,
};
use reth_interfaces::provider::ProviderResult;
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    snapshot::{
        find_fixed_range, Compression, Filters, InclusionFilter, PerfectHashingFunction,
        SegmentConfig, SegmentHeader,
    },
    BlockNumber, SnapshotSegment,
};
use reth_provider::{providers::SnapshotProvider, DatabaseProviderRO, TransactionsProviderExt};
use std::{ops::RangeInclusive, path::Path, sync::Arc};

pub(crate) type Rows<const COLUMNS: usize> = [Vec<Vec<u8>>; COLUMNS];

/// A segment represents a snapshotting of some portion of the data.
pub trait Segment<DB: Database>: Send + Sync {
    /// Returns the [`SnapshotSegment`].
    fn segment(&self) -> SnapshotSegment;

    /// Snapshot data for the provided block range. [SnapshotProvider] will handle the management of
    /// and writing to files.
    fn snapshot(
        &self,
        provider: DatabaseProviderRO<DB>,
        snapshot_provider: Arc<SnapshotProvider>,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()>;

    /// Create a snapshot file of data for the provided block range. The `directory` parameter
    /// determines the snapshot file's save location.
    fn create_snapshot_file(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: &Path,
        config: SegmentConfig,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()>;
}

/// Returns a [`NippyJar`] according to the desired configuration. The `directory` parameter
/// determines the snapshot file's save location.
pub(crate) fn prepare_jar<DB: Database, const COLUMNS: usize>(
    provider: &DatabaseProviderRO<DB>,
    directory: impl AsRef<Path>,
    segment: SnapshotSegment,
    segment_config: SegmentConfig,
    block_range: RangeInclusive<BlockNumber>,
    total_rows: usize,
    prepare_compression: impl Fn() -> ProviderResult<Rows<COLUMNS>>,
) -> ProviderResult<NippyJar<SegmentHeader>> {
    let tx_range = match segment {
        SnapshotSegment::Headers => None,
        SnapshotSegment::Receipts | SnapshotSegment::Transactions => {
            Some(provider.transaction_range_by_block_range(block_range.clone())?)
        }
    };

    let mut nippy_jar = NippyJar::new(
        COLUMNS,
        &directory.as_ref().join(segment.filename(&find_fixed_range(*block_range.end())).as_str()),
        SegmentHeader::new(block_range, tx_range, segment),
    );

    nippy_jar = match segment_config.compression {
        Compression::Lz4 => nippy_jar.with_lz4(),
        Compression::Zstd => nippy_jar.with_zstd(false, 0),
        Compression::ZstdWithDictionary => {
            let dataset = prepare_compression()?;

            nippy_jar = nippy_jar.with_zstd(true, 5_000_000);
            nippy_jar.prepare_compression(dataset.to_vec())?;
            nippy_jar
        }
        Compression::Uncompressed => nippy_jar,
    };

    if let Filters::WithFilters(inclusion_filter, phf) = segment_config.filters {
        nippy_jar = match inclusion_filter {
            InclusionFilter::Cuckoo => nippy_jar.with_cuckoo_filter(total_rows),
        };
        nippy_jar = match phf {
            PerfectHashingFunction::Fmph => nippy_jar.with_fmph(),
            PerfectHashingFunction::GoFmph => nippy_jar.with_gofmph(),
        };
    }

    Ok(nippy_jar)
}

/// Generates the dataset to train a zstd dictionary with the most recent rows (at most 1000).
pub(crate) fn dataset_for_compression<DB: Database, T: Table<Key = u64>>(
    provider: &DatabaseProviderRO<DB>,
    range: &RangeInclusive<u64>,
    range_len: usize,
) -> ProviderResult<Vec<Vec<u8>>> {
    let mut cursor = provider.tx_ref().cursor_read::<RawTable<T>>()?;
    Ok(cursor
        .walk_back(Some(RawKey::from(*range.end())))?
        .take(range_len.min(1000))
        .map(|row| row.map(|(_key, value)| value.into_value()).expect("should exist"))
        .collect::<Vec<_>>())
}
