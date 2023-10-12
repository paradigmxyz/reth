//! Snapshot segment implementations and utilities.

mod headers;

pub use headers::Headers;

use reth_db::{table::Table, transaction::DbTx};
use reth_interfaces::RethResult;
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    snapshot::{Compression, Filters, InclusionFilter, PerfectHashingFunction},
    BlockNumber, SnapshotSegment,
};
use std::{ops::RangeInclusive, path::PathBuf};

pub(crate) type Rows<const COLUMNS: usize> = [Vec<Vec<u8>>; COLUMNS];

/// A segment represents a snapshotting of some portion of the data.
pub trait Segment {
    /// Snapshot data using the provided range.
    fn snapshot<'tx>(
        &self,
        tx: &impl DbTx<'tx>,
        range: RangeInclusive<BlockNumber>,
    ) -> RethResult<()>;
}

/// Returns a [`NippyJar`] according to the desired configuration.
pub(crate) fn prepare_jar<'tx, const COLUMNS: usize, T: Table>(
    tx: &impl DbTx<'tx>,
    segment: SnapshotSegment,
    filters: Filters,
    compression: Compression,
    range: RangeInclusive<BlockNumber>,
    range_len: usize,
    prepare_compression: impl Fn() -> RethResult<Rows<COLUMNS>>,
) -> RethResult<NippyJar> {
    let mut nippy_jar = NippyJar::new_without_header(
        COLUMNS,
        &get_snapshot_segment_file_name(segment, filters, compression, &range),
    );

    nippy_jar = match compression {
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

    if let Filters::WithFilters(inclusion_filter, phf) = filters {
        let total_rows = (tx.entries::<T>()? - *range.start() as usize).min(range_len);
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

/// Returns file name for the provided segment, filters, compression and range.
pub fn get_snapshot_segment_file_name(
    segment: SnapshotSegment,
    filters: Filters,
    compression: Compression,
    range: &RangeInclusive<BlockNumber>,
) -> PathBuf {
    let segment_name = match segment {
        SnapshotSegment::Headers => "headers",
        SnapshotSegment::Transactions => "transactions",
        SnapshotSegment::Receipts => "receipts",
    };
    let filters_name = match filters {
        Filters::WithFilters(inclusion_filter, phf) => {
            let inclusion_filter = match inclusion_filter {
                InclusionFilter::Cuckoo => "cuckoo",
            };
            let phf = match phf {
                PerfectHashingFunction::Fmph => "fmph",
                PerfectHashingFunction::GoFmph => "gofmph",
            };
            format!("{inclusion_filter}-{phf}")
        }
        Filters::WithoutFilters => "none".to_string(),
    };
    let compression_name = match compression {
        Compression::Lz4 => "lz4",
        Compression::Zstd => "zstd",
        Compression::ZstdWithDictionary => "zstd-dict",
        Compression::Uncompressed => "uncompressed",
    };

    format!(
        "snapshot_{segment_name}_{}_{}_{filters_name}_{compression_name}",
        range.start(),
        range.end(),
    )
    .into()
}
