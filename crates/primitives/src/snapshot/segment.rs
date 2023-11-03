use crate::{
    snapshot::{Compression, Filters, InclusionFilter},
    BlockNumber, TxNumber,
};
use serde::{Deserialize, Serialize};
use std::{ops::RangeInclusive, path::PathBuf, str::FromStr};
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
        let segment_name = self.as_ref();

        let filters_name = match filters {
            Filters::WithFilters(inclusion_filter, phf) => {
                format!("{}-{}", inclusion_filter.as_ref(), phf.as_ref())
            }
            Filters::WithoutFilters => "none".to_string(),
        };

        // ATTENTION: if changing the name format, be sure to reflect those changes in
        // [`Self::parse_filename`.]
        format!(
            "snapshot_{segment_name}_{}_{}_{}_{}",
            range.start(),
            range.end(),
            filters_name,
            compression.as_ref()
        )
        .into()
    }

    /// Takes a filename and parses the [`SnapshotSegment`] and its inclusive range.
    pub fn parse_filename(name: &str) -> Option<(Self, RangeInclusive<BlockNumber>)> {
        let parts: Vec<&str> = name.split('_').collect();
        if let (Ok(segment), true) = (Self::from_str(parts[1]), parts.len() >= 4) {
            let start: u64 = parts[2].parse().unwrap_or(0);
            let end: u64 = parts[3].parse().unwrap_or(0);

            if start <= end || parts[0] != "snapshot" {
                return None
            }

            return Some((segment, start..=end))
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
