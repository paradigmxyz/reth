use crate::{
    static_file::{Compression, Filters, InclusionFilter},
    BlockNumber, TxNumber,
};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::{ops::RangeInclusive, str::FromStr};
use strum::{AsRefStr, EnumIter, EnumString};

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
    EnumIter,
    AsRefStr,
    Display,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Segment of the data that can be moved to static files.
pub enum StaticFileSegment {
    #[strum(serialize = "headers")]
    /// Static File segment responsible for the `CanonicalHeaders`, `Headers`,
    /// `HeaderTerminalDifficulties` tables.
    Headers,
    #[strum(serialize = "transactions")]
    /// Static File segment responsible for the `Transactions` table.
    Transactions,
    #[strum(serialize = "receipts")]
    /// Static File segment responsible for the `Receipts` table.
    Receipts,
}

impl StaticFileSegment {
    /// Returns the segment as a string.
    pub const fn as_str(&self) -> &'static str {
        match self {
            StaticFileSegment::Headers => "headers",
            StaticFileSegment::Transactions => "transactions",
            StaticFileSegment::Receipts => "receipts",
        }
    }

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
            StaticFileSegment::Headers => default_config,
            StaticFileSegment::Transactions => default_config,
            StaticFileSegment::Receipts => default_config,
        }
    }

    /// Returns the number of columns for the segment
    pub const fn columns(&self) -> usize {
        match self {
            StaticFileSegment::Headers => 3,
            StaticFileSegment::Transactions => 1,
            StaticFileSegment::Receipts => 1,
        }
    }

    /// Returns the default file name for the provided segment and range.
    pub fn filename(&self, block_range: &SegmentRangeInclusive) -> String {
        // ATTENTION: if changing the name format, be sure to reflect those changes in
        // [`Self::parse_filename`].
        format!("static_file_{}_{}_{}", self.as_ref(), block_range.start(), block_range.end())
    }

    /// Returns file name for the provided segment and range, alongside filters, compression.
    pub fn filename_with_configuration(
        &self,
        filters: Filters,
        compression: Compression,
        block_range: &SegmentRangeInclusive,
    ) -> String {
        let prefix = self.filename(block_range);

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

    /// Parses a filename into a `StaticFileSegment` and its expected block range.
    ///
    /// The filename is expected to follow the format:
    /// "static_file_{segment}_{block_start}_{block_end}". This function checks
    /// for the correct prefix ("static_file"), and then parses the segment and the inclusive
    /// ranges for blocks. It ensures that the start of each range is less than or equal to the
    /// end.
    ///
    /// # Returns
    /// - `Some((segment, block_range))` if parsing is successful and all conditions are met.
    /// - `None` if any condition fails, such as an incorrect prefix, parsing error, or invalid
    ///   range.
    ///
    /// # Note
    /// This function is tightly coupled with the naming convention defined in [`Self::filename`].
    /// Any changes in the filename format in `filename` should be reflected here.
    pub fn parse_filename(name: &str) -> Option<(Self, SegmentRangeInclusive)> {
        let mut parts = name.split('_');
        if !(parts.next() == Some("static") && parts.next() == Some("file")) {
            return None
        }

        let segment = Self::from_str(parts.next()?).ok()?;
        let (block_start, block_end) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);

        if block_start > block_end {
            return None
        }

        Some((segment, SegmentRangeInclusive::new(block_start, block_end)))
    }

    /// Returns `true` if the segment is `StaticFileSegment::Headers`.
    pub fn is_headers(&self) -> bool {
        matches!(self, StaticFileSegment::Headers)
    }
}

/// A segment header that contains information common to all segments. Used for storage.
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash, Clone)]
pub struct SegmentHeader {
    /// Defines the expected block range for a static file segment. This attribute is crucial for
    /// scenarios where the file contains no data, allowing for a representation beyond a
    /// simple `start..=start` range. It ensures clarity in differentiating between an empty file
    /// and a file with a single block numbered 0.
    expected_block_range: SegmentRangeInclusive,
    /// Block range of data on the static file segment
    block_range: Option<SegmentRangeInclusive>,
    /// Transaction range of data of the static file segment
    tx_range: Option<SegmentRangeInclusive>,
    /// Segment type
    segment: StaticFileSegment,
}

impl SegmentHeader {
    /// Returns [`SegmentHeader`].
    pub fn new(
        expected_block_range: SegmentRangeInclusive,
        block_range: Option<SegmentRangeInclusive>,
        tx_range: Option<SegmentRangeInclusive>,
        segment: StaticFileSegment,
    ) -> Self {
        Self { expected_block_range, block_range, tx_range, segment }
    }

    /// Returns the static file segment kind.
    pub fn segment(&self) -> StaticFileSegment {
        self.segment
    }

    /// Returns the block range.
    pub fn block_range(&self) -> Option<&SegmentRangeInclusive> {
        self.block_range.as_ref()
    }

    /// Returns the transaction range.
    pub fn tx_range(&self) -> Option<&SegmentRangeInclusive> {
        self.tx_range.as_ref()
    }

    /// The expected block start of the segment.
    pub fn expected_block_start(&self) -> BlockNumber {
        self.expected_block_range.start()
    }

    /// The expected block end of the segment.
    pub fn expected_block_end(&self) -> BlockNumber {
        self.expected_block_range.end()
    }

    /// Returns the first block number of the segment.
    pub fn block_start(&self) -> Option<BlockNumber> {
        self.block_range.as_ref().map(|b| b.start())
    }

    /// Returns the last block number of the segment.
    pub fn block_end(&self) -> Option<BlockNumber> {
        self.block_range.as_ref().map(|b| b.end())
    }

    /// Returns the first transaction number of the segment.  
    pub fn tx_start(&self) -> Option<TxNumber> {
        self.tx_range.as_ref().map(|t| t.start())
    }

    /// Returns the last transaction number of the segment.   
    pub fn tx_end(&self) -> Option<TxNumber> {
        self.tx_range.as_ref().map(|t| t.end())
    }

    /// Number of transactions.  
    pub fn tx_len(&self) -> Option<u64> {
        self.tx_range.as_ref().map(|r| (r.end() + 1) - r.start())
    }

    /// Number of blocks.
    pub fn block_len(&self) -> Option<u64> {
        self.block_range.as_ref().map(|r| (r.end() + 1) - r.start())
    }

    /// Increments block end range depending on segment
    pub fn increment_block(&mut self) -> BlockNumber {
        if let Some(block_range) = &mut self.block_range {
            block_range.end += 1;
            block_range.end
        } else {
            self.block_range = Some(SegmentRangeInclusive::new(
                self.expected_block_start(),
                self.expected_block_start(),
            ));
            self.expected_block_start()
        }
    }

    /// Increments tx end range depending on segment
    pub fn increment_tx(&mut self) {
        match self.segment {
            StaticFileSegment::Headers => (),
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                if let Some(tx_range) = &mut self.tx_range {
                    tx_range.end += 1;
                } else {
                    self.tx_range = Some(SegmentRangeInclusive::new(0, 0));
                }
            }
        }
    }

    /// Removes `num` elements from end of tx or block range.
    pub fn prune(&mut self, num: u64) {
        match self.segment {
            StaticFileSegment::Headers => {
                if let Some(range) = &mut self.block_range {
                    if num > range.end {
                        self.block_range = None;
                    } else {
                        range.end = range.end.saturating_sub(num);
                    }
                };
            }
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => {
                if let Some(range) = &mut self.tx_range {
                    if num > range.end {
                        self.tx_range = None;
                    } else {
                        range.end = range.end.saturating_sub(num);
                    }
                };
            }
        };
    }

    /// Sets a new block_range.
    pub fn set_block_range(&mut self, block_start: BlockNumber, block_end: BlockNumber) {
        if let Some(block_range) = &mut self.block_range {
            block_range.start = block_start;
            block_range.end = block_end;
        } else {
            self.block_range = Some(SegmentRangeInclusive::new(block_start, block_end))
        }
    }

    /// Sets a new tx_range.
    pub fn set_tx_range(&mut self, tx_start: TxNumber, tx_end: TxNumber) {
        if let Some(tx_range) = &mut self.tx_range {
            tx_range.start = tx_start;
            tx_range.end = tx_end;
        } else {
            self.tx_range = Some(SegmentRangeInclusive::new(tx_start, tx_end))
        }
    }

    /// Returns the row offset which depends on whether the segment is block or transaction based.
    pub fn start(&self) -> Option<u64> {
        match self.segment {
            StaticFileSegment::Headers => self.block_start(),
            StaticFileSegment::Transactions | StaticFileSegment::Receipts => self.tx_start(),
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

/// Helper type to handle segment transaction and block INCLUSIVE ranges.
///
/// They can be modified on a hot loop, which makes the `std::ops::RangeInclusive` a poor fit.
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash, Clone, Copy)]
pub struct SegmentRangeInclusive {
    start: u64,
    end: u64,
}

impl SegmentRangeInclusive {
    /// Creates a new [`SegmentRangeInclusive`]
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// Start of the inclusive range
    pub fn start(&self) -> u64 {
        self.start
    }

    /// End of the inclusive range
    pub fn end(&self) -> u64 {
        self.end
    }
}

impl std::fmt::Display for SegmentRangeInclusive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..={}", self.start, self.end)
    }
}

impl From<RangeInclusive<u64>> for SegmentRangeInclusive {
    fn from(value: RangeInclusive<u64>) -> Self {
        SegmentRangeInclusive { start: *value.start(), end: *value.end() }
    }
}

impl From<&SegmentRangeInclusive> for RangeInclusive<u64> {
    fn from(value: &SegmentRangeInclusive) -> Self {
        value.start()..=value.end()
    }
}

impl From<SegmentRangeInclusive> for RangeInclusive<u64> {
    fn from(value: SegmentRangeInclusive) -> Self {
        (&value).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filename() {
        let test_vectors = [
            (StaticFileSegment::Headers, 2..=30, "static_file_headers_2_30", None),
            (StaticFileSegment::Receipts, 30..=300, "static_file_receipts_30_300", None),
            (
                StaticFileSegment::Transactions,
                1_123_233..=11_223_233,
                "static_file_transactions_1123233_11223233",
                None,
            ),
            (
                StaticFileSegment::Headers,
                2..=30,
                "static_file_headers_2_30_cuckoo-fmph_lz4",
                Some((
                    Compression::Lz4,
                    Filters::WithFilters(
                        InclusionFilter::Cuckoo,
                        crate::static_file::PerfectHashingFunction::Fmph,
                    ),
                )),
            ),
            (
                StaticFileSegment::Headers,
                2..=30,
                "static_file_headers_2_30_cuckoo-fmph_zstd",
                Some((
                    Compression::Zstd,
                    Filters::WithFilters(
                        InclusionFilter::Cuckoo,
                        crate::static_file::PerfectHashingFunction::Fmph,
                    ),
                )),
            ),
            (
                StaticFileSegment::Headers,
                2..=30,
                "static_file_headers_2_30_cuckoo-fmph_zstd-dict",
                Some((
                    Compression::ZstdWithDictionary,
                    Filters::WithFilters(
                        InclusionFilter::Cuckoo,
                        crate::static_file::PerfectHashingFunction::Fmph,
                    ),
                )),
            ),
        ];

        for (segment, block_range, filename, configuration) in test_vectors {
            let block_range: SegmentRangeInclusive = block_range.into();
            if let Some((compression, filters)) = configuration {
                assert_eq!(
                    segment.filename_with_configuration(filters, compression, &block_range,),
                    filename
                );
            } else {
                assert_eq!(segment.filename(&block_range), filename);
            }

            assert_eq!(StaticFileSegment::parse_filename(filename), Some((segment, block_range)));
        }

        assert_eq!(StaticFileSegment::parse_filename("static_file_headers_2"), None);
        assert_eq!(StaticFileSegment::parse_filename("static_file_headers_"), None);
    }
}
