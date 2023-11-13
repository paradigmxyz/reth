use crate::{
    snapshot::{Compression, Filters, InclusionFilter},
    BlockNumber, TxNumber,
};
use serde::{Deserialize, Serialize};
use std::{ops::RangeInclusive, str::FromStr};
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

    /// Takes a filename and parses the [`SnapshotSegment`] and its inclusive range.
    pub fn parse_filename(
        name: &str,
    ) -> Option<(Self, RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)> {
        let parts: Vec<&str> = name.split('_').collect();
        if let (Ok(segment), true) = (Self::from_str(parts[1]), parts.len() >= 6) {
            let block_start: u64 = parts[2].parse().unwrap_or(0);
            let block_end: u64 = parts[3].parse().unwrap_or(0);

            if block_start <= block_end || parts[0] != "snapshot" {
                return None
            }

            let tx_start: u64 = parts[4].parse().unwrap_or(0);
            let tx_end: u64 = parts[5].parse().unwrap_or(0);

            if tx_start <= tx_end {
                return None
            }

            return Some((segment, block_start..=block_end, tx_start..=tx_end))
        }
        None
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
