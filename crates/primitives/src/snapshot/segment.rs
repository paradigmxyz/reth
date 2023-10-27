use crate::{snapshot::PerfectHashingFunction, BlockNumber, TxNumber};
use serde::{Deserialize, Serialize};
use std::{ops::RangeInclusive, path::PathBuf};

use super::{Compression, Filters, InclusionFilter};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Segment of the data that can be snapshotted.
pub enum SnapshotSegment {
    /// Snapshot segment responsible for the `CanonicalHeaders`, `Headers`, `HeaderTD` tables.
    Headers,
    /// Snapshot segment responsible for the `Transactions` table.
    Transactions,
    /// Snapshot segment responsible for the `Receipts` table.
    Receipts,
}

impl SnapshotSegment {
    /// Returns the default configuration of the segment.
    const fn config(&self) -> (Filters, Compression) {
        let default_config = (
            Filters::WithFilters(InclusionFilter::Cuckoo, super::PerfectHashingFunction::Fmph),
            Compression::Lz4,
        );

        match self {
            SnapshotSegment::Headers => default_config,
            SnapshotSegment::Transactions => default_config,
            SnapshotSegment::Receipts => default_config,
        }
    }

    /// Returns the default file name for the provided segment and range.
    pub fn filename(&self, range: &RangeInclusive<BlockNumber>) -> PathBuf {
        let (filters, compression) = self.config();
        self.filename_with_configuration(filters, compression, range)
    }

    /// Returns file name for the provided segment, filters, compression and range.
    pub fn filename_with_configuration(
        &self,
        filters: Filters,
        compression: Compression,
        range: &RangeInclusive<BlockNumber>,
    ) -> PathBuf {
        let segment_name = match self {
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
}

/// A segment header that contains information common to all segments. Used for storage.
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct SegmentHeader {
    block_range: RangeInclusive<BlockNumber>,
    tx_range: RangeInclusive<TxNumber>,
}

impl SegmentHeader {
    /// Returns [`SegmentHeader`].
    pub fn new(
        block_range: RangeInclusive<BlockNumber>,
        tx_range: RangeInclusive<TxNumber>,
    ) -> Self {
        Self { block_range, tx_range }
    }

    /// Returns the first block number of the segment.
    pub fn block_start(&self) -> BlockNumber {
        *self.block_range.start()
    }

    /// Returns the first transaction number of the segment.
    pub fn tx_start(&self) -> TxNumber {
        *self.tx_range.start()
    }
}
