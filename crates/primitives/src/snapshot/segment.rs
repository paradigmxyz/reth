use crate::{
    snapshot::{Compression, Filters, InclusionFilter},
    BlockNumber, TxNumber,
};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::{ffi::OsStr, ops::RangeInclusive, str::FromStr};
use strum::{AsRefStr, EnumString};

#[derive(
    Debug,
    Copy,
    Clone,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Deserialize,
    Serialize,
    EnumString,
    AsRefStr,
    Display,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Segment of the data that can be snapshotted.
pub enum SnapshotSegment {
    #[strum(serialize = "headers")]
    /// Snapshot segment responsible for the `CanonicalHeaders`, `Headers`, `HeaderTD` tables.
    Headers,
    #[strum(serialize = "transactions")]
    /// Snapshot segment responsible for the `Transactions` table.
    Transactions,
    #[strum(serialize = "receipts")]
    /// Snapshot segment responsible for the `Receipts` table.
    Receipts,
}

impl SnapshotSegment {
    /// Returns the default configuration of the segment.
    pub const fn config(&self) -> SegmentConfig {
        let default_config = SegmentConfig {
            filters: Filters::WithFilters(
                InclusionFilter::Cuckoo,
                super::PerfectHashingFunction::Fmph,
            ),
            compression: Compression::Lz4,
        };

        match self {
            SnapshotSegment::Headers => default_config,
            SnapshotSegment::Transactions => default_config,
            SnapshotSegment::Receipts => default_config,
        }
    }

    /// Returns the default file name for the provided segment and range.
    pub fn filename(
        &self,
        block_range: &RangeInclusive<BlockNumber>,
        tx_range: &RangeInclusive<TxNumber>,
    ) -> String {
        // ATTENTION: if changing the name format, be sure to reflect those changes in
        // [`Self::parse_filename`].
        format!(
            "snapshot_{}_{}_{}_{}_{}",
            self.as_ref(),
            block_range.start(),
            block_range.end(),
            tx_range.start(),
            tx_range.end(),
        )
    }

    /// Returns file name for the provided segment and range, alongisde filters, compression.
    pub fn filename_with_configuration(
        &self,
        filters: Filters,
        compression: Compression,
        block_range: &RangeInclusive<BlockNumber>,
        tx_range: &RangeInclusive<TxNumber>,
    ) -> String {
        let prefix = self.filename(block_range, tx_range);

        let filters_name = match filters {
            Filters::WithFilters(inclusion_filter, phf) => {
                format!("{}-{}", inclusion_filter.as_ref(), phf.as_ref())
            }
            Filters::WithoutFilters => "none".to_string(),
        };

        // ATTENTION: if changing the name format, be sure to reflect those changes in
        // [`Self::parse_filename`.]
        format!("{prefix}_{}_{}", filters_name, compression.as_ref())
    }

    /// Parses a filename into a `SnapshotSegment` and its corresponding block and transaction
    /// ranges.
    ///
    /// The filename is expected to follow the format:
    /// "snapshot_{segment}_{block_start}_{block_end}_{tx_start}_{tx_end}". This function checks
    /// for the correct prefix ("snapshot"), and then parses the segment and the inclusive
    /// ranges for blocks and transactions. It ensures that the start of each range is less than the
    /// end.
    ///
    /// # Returns
    /// - `Some((segment, block_range, tx_range))` if parsing is successful and all conditions are
    ///   met.
    /// - `None` if any condition fails, such as an incorrect prefix, parsing error, or invalid
    ///   range.
    ///
    /// # Note
    /// This function is tightly coupled with the naming convention defined in [`Self::filename`].
    /// Any changes in the filename format in `filename` should be reflected here.
    pub fn parse_filename(
        name: &OsStr,
    ) -> Option<(Self, RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)> {
        let mut parts = name.to_str()?.split('_');
        if parts.next() != Some("snapshot") {
            return None
        }

        let segment = Self::from_str(parts.next()?).ok()?;
        let (block_start, block_end) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        let (tx_start, tx_end) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);

        if block_start >= block_end || tx_start > tx_end {
            return None
        }

        Some((segment, block_start..=block_end, tx_start..=tx_end))
    }
}

/// A segment header that contains information common to all segments. Used for storage.
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct SegmentHeader {
    /// Block range of the snapshot segment
    block_range: RangeInclusive<BlockNumber>,
    /// Transaction range of the snapshot segment
    tx_range: RangeInclusive<TxNumber>,
    /// Segment type
    segment: SnapshotSegment,
}

impl SegmentHeader {
    /// Returns [`SegmentHeader`].
    pub fn new(
        block_range: RangeInclusive<BlockNumber>,
        tx_range: RangeInclusive<TxNumber>,
        segment: SnapshotSegment,
    ) -> Self {
        Self { block_range, tx_range, segment }
    }

    /// Returns the first block number of the segment.
    pub fn block_start(&self) -> BlockNumber {
        *self.block_range.start()
    }

    /// Returns the last block number of the segment.
    pub fn block_end(&self) -> BlockNumber {
        *self.block_range.end()
    }

    /// Returns the first transaction number of the segment.
    pub fn tx_start(&self) -> TxNumber {
        *self.tx_range.start()
    }

    /// Returns the row offset which depends on whether the segment is block or transaction based.
    pub fn start(&self) -> u64 {
        match self.segment {
            SnapshotSegment::Headers => self.block_start(),
            SnapshotSegment::Transactions | SnapshotSegment::Receipts => self.tx_start(),
        }
    }
}

/// Configuration used on the segment.
#[derive(Debug, Clone, Copy)]
pub struct SegmentConfig {
    /// Inclusion filters used on the segment
    pub filters: Filters,
    /// Compression used on the segment
    pub compression: Compression,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filename() {
        let test_vectors = [
            (SnapshotSegment::Headers, 2..=30, 0..=1, "snapshot_headers_2_30_0_1", None),
            (
                SnapshotSegment::Receipts,
                30..=300,
                110..=1000,
                "snapshot_receipts_30_300_110_1000",
                None,
            ),
            (
                SnapshotSegment::Transactions,
                1_123_233..=11_223_233,
                1_123_233..=2_123_233,
                "snapshot_transactions_1123233_11223233_1123233_2123233",
                None,
            ),
            (
                SnapshotSegment::Headers,
                2..=30,
                0..=1,
                "snapshot_headers_2_30_0_1_cuckoo-fmph_lz4",
                Some((
                    Compression::Lz4,
                    Filters::WithFilters(
                        InclusionFilter::Cuckoo,
                        crate::snapshot::PerfectHashingFunction::Fmph,
                    ),
                )),
            ),
            (
                SnapshotSegment::Headers,
                2..=30,
                0..=1,
                "snapshot_headers_2_30_0_1_cuckoo-fmph_zstd",
                Some((
                    Compression::Zstd,
                    Filters::WithFilters(
                        InclusionFilter::Cuckoo,
                        crate::snapshot::PerfectHashingFunction::Fmph,
                    ),
                )),
            ),
            (
                SnapshotSegment::Headers,
                2..=30,
                0..=1,
                "snapshot_headers_2_30_0_1_cuckoo-fmph_zstd-dict",
                Some((
                    Compression::ZstdWithDictionary,
                    Filters::WithFilters(
                        InclusionFilter::Cuckoo,
                        crate::snapshot::PerfectHashingFunction::Fmph,
                    ),
                )),
            ),
        ];

        for (segment, block_range, tx_range, filename, configuration) in test_vectors {
            if let Some((compression, filters)) = configuration {
                assert_eq!(
                    segment.filename_with_configuration(
                        filters,
                        compression,
                        &block_range,
                        &tx_range
                    ),
                    filename
                );
            } else {
                assert_eq!(segment.filename(&block_range, &tx_range), filename);
            }

            assert_eq!(
                SnapshotSegment::parse_filename(OsStr::new(filename)),
                Some((segment, block_range, tx_range))
            );
        }

        assert_eq!(SnapshotSegment::parse_filename(OsStr::new("snapshot_headers_2_30_3_2")), None);
        assert_eq!(SnapshotSegment::parse_filename(OsStr::new("snapshot_headers_2_30_1")), None);
    }
}
