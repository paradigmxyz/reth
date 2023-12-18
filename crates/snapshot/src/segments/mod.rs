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
        Compression, Filters, InclusionFilter, PerfectHashingFunction, SegmentConfig, SegmentHeader,
    },
    BlockNumber, SnapshotSegment,
};
use reth_provider::{DatabaseProviderRO, TransactionsProviderExt};
use std::{ops::RangeInclusive, path::Path};

pub(crate) type Rows<const COLUMNS: usize> = [Vec<Vec<u8>>; COLUMNS];

/// A segment represents a snapshotting of some portion of the data.
pub trait Segment: Default {
    /// Snapshot data using the provided range. The `directory` parameter determines the snapshot
    /// file's save location.
    fn snapshot<DB: Database>(
        &self,
        provider: &DatabaseProviderRO<DB>,
        directory: impl AsRef<Path>,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<()>;

    /// Returns this struct's [`SnapshotSegment`].
    fn segment(&self) -> SnapshotSegment;

    /// Generates the dataset to train a zstd dictionary with the most recent rows (at most 1000).
    fn dataset_for_compression<DB: Database, T: Table<Key = u64>>(
        &self,
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
    let tx_range = provider.transaction_range_by_block_range(block_range.clone())?;
    let mut nippy_jar = NippyJar::new(
        COLUMNS,
        &directory.as_ref().join(segment.filename(&block_range, &tx_range).as_str()),
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
