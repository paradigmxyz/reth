use crate::{BlockNumber, Compression};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use alloy_primitives::TxNumber;
use core::{
    ops::{Range, RangeInclusive},
    str::FromStr,
};
use derive_more::Display;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
    EnumString,
    AsRefStr,
    Display,
    Serialize,
    Deserialize,
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
    #[strum(serialize = "accountchangesets")]
    /// Static File segment responsible for the `AccountChangeSets` table.
    AccountChangeSets,
}

impl StaticFileSegment {
    /// Returns the segment as a string.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Headers => "headers",
            Self::Transactions => "transactions",
            Self::Receipts => "receipts",
            Self::AccountChangeSets => "account_change_sets",
        }
    }

    /// Returns an iterator over all segments.
    pub fn iter() -> impl Iterator<Item = Self> {
        // The order of segments is significant and must be maintained to ensure correctness.
        [Self::Headers, Self::Transactions, Self::Receipts, Self::AccountChangeSets].into_iter()
    }

    /// Returns the default configuration of the segment.
    pub const fn config(&self) -> SegmentConfig {
        SegmentConfig { compression: Compression::Lz4 }
    }

    /// Returns the number of columns for the segment
    pub const fn columns(&self) -> usize {
        match self {
            Self::Headers => 3,
            Self::Transactions | Self::Receipts | Self::AccountChangeSets => 1,
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
        compression: Compression,
        block_range: &SegmentRangeInclusive,
    ) -> String {
        let prefix = self.filename(block_range);

        let filters_name = "none".to_string();

        // ATTENTION: if changing the name format, be sure to reflect those changes in
        // [`Self::parse_filename`.]
        format!("{prefix}_{}_{}", filters_name, compression.as_ref())
    }

    /// Parses a filename into a `StaticFileSegment` and its expected block range.
    ///
    /// The filename is expected to follow the format:
    /// "`static_file`_{segment}_{`block_start`}_{`block_end`}". This function checks
    /// for the correct prefix ("`static_file`"), and then parses the segment and the inclusive
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
    pub const fn is_headers(&self) -> bool {
        matches!(self, Self::Headers)
    }

    /// Returns `true` if the segment is `StaticFileSegment::Receipts`.
    pub const fn is_receipts(&self) -> bool {
        matches!(self, Self::Receipts)
    }

    /// Returns `true` if a segment row is linked to a transaction.
    pub const fn is_tx_based(&self) -> bool {
        matches!(self, Self::Receipts | Self::Transactions)
    }

    /// Returns `true` if the segment is `StaticFileSegment::AccountChangeSets`
    pub const fn is_account_changesets(&self) -> bool {
        matches!(self, Self::AccountChangeSets)
    }

    /// Returns `true` if a segment row is linked to a block.
    pub const fn is_block_based(&self) -> bool {
        // TODO(rjected): figure out if this is correct for account change sets
        matches!(self, Self::Headers | Self::AccountChangeSets)
    }
}

/// A changeset offset, also with the number of elements in the offset for convenience
// TODO: should this be replaced with a method in SegmentHeader?
#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash, Clone)]
pub struct ChangesetOffset {
    /// Offset for the row for this block
    offset: u64,

    /// Number of changes in this changeset
    num_changes: u64,
}

impl ChangesetOffset {
    /// Returns the start offset for the row for this block
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the number of changes in this changeset
    pub const fn num_changes(&self) -> u64 {
        self.num_changes
    }

    /// Returns a range corresponding to the changes.
    pub const fn changeset_range(&self) -> Range<u64> {
        self.offset..(self.offset + self.num_changes)
    }
}

/// A segment header that contains information common to all segments. Used for storage.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
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
    /// List of offsets, for where each block's changeset starts.
    changeset_offsets: Option<Vec<ChangesetOffset>>,
}

impl Serialize for SegmentHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // For backward compatibility, only serialize 4 fields (without changeset_offsets)
        // This matches what we deserialize from old files
        (&self.expected_block_range, &self.block_range, &self.tx_range, &self.segment)
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SegmentHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Old format without changeset_offsets (for backward compatibility)
        #[derive(Deserialize)]
        struct OldHeader {
            expected_block_range: SegmentRangeInclusive,
            block_range: Option<SegmentRangeInclusive>,
            tx_range: Option<SegmentRangeInclusive>,
            segment: StaticFileSegment,
        }

        // For existing files created before changeset_offsets was added
        let header = OldHeader::deserialize(deserializer)?;

        Ok(Self {
            expected_block_range: header.expected_block_range,
            block_range: header.block_range,
            tx_range: header.tx_range,
            segment: header.segment,
            changeset_offsets: None,
        })
    }
}

impl SegmentHeader {
    /// Returns [`SegmentHeader`].
    pub const fn new(
        expected_block_range: SegmentRangeInclusive,
        block_range: Option<SegmentRangeInclusive>,
        tx_range: Option<SegmentRangeInclusive>,
        segment: StaticFileSegment,
    ) -> Self {
        Self { expected_block_range, block_range, tx_range, segment, changeset_offsets: None }
    }

    /// Returns the static file segment kind.
    pub const fn segment(&self) -> StaticFileSegment {
        self.segment
    }

    /// Returns the block range.
    pub const fn block_range(&self) -> Option<&SegmentRangeInclusive> {
        self.block_range.as_ref()
    }

    /// Returns the transaction range.
    pub const fn tx_range(&self) -> Option<&SegmentRangeInclusive> {
        self.tx_range.as_ref()
    }

    /// Returns the changeset offsets.
    pub const fn changeset_offsets(&self) -> Option<&Vec<ChangesetOffset>> {
        self.changeset_offsets.as_ref()
    }

    /// The expected block start of the segment.
    pub const fn expected_block_start(&self) -> BlockNumber {
        self.expected_block_range.start()
    }

    /// The expected block end of the segment.
    pub const fn expected_block_end(&self) -> BlockNumber {
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
        let block_num = if let Some(block_range) = &mut self.block_range {
            block_range.end += 1;
            block_range.end
        } else {
            self.block_range = Some(SegmentRangeInclusive::new(
                self.expected_block_start(),
                self.expected_block_start(),
            ));
            self.expected_block_start()
        };

        // For changeset segments, initialize an offset entry for the new block
        if self.segment.is_account_changesets() {
            let offsets = self.changeset_offsets.get_or_insert_with(Default::default);
            // Calculate the offset for the new block
            let new_offset = if let Some(last_offset) = offsets.last() {
                // The new block starts after the last block's changes
                last_offset.offset + last_offset.num_changes
            } else {
                // First block starts at offset 0
                0
            };
            // Add a new offset entry with 0 changes initially
            offsets.push(ChangesetOffset { offset: new_offset, num_changes: 0 });
        }

        block_num
    }

    /// Increments tx end range depending on segment
    pub const fn increment_tx(&mut self) {
        if self.segment.is_tx_based() {
            if let Some(tx_range) = &mut self.tx_range {
                tx_range.end += 1;
            } else {
                self.tx_range = Some(SegmentRangeInclusive::new(0, 0));
            }
        }
    }

    /// Increments the latest block's number of changes.
    pub fn increment_block_changes(&mut self) {
        if self.segment.is_account_changesets() {
            let offsets = self.changeset_offsets.get_or_insert_with(Default::default);
            if let Some(last_offset) = offsets.last_mut() {
                last_offset.num_changes += 1;
            } else {
                // If offsets is empty, we are adding the first change for a block
                // The offset for the first block is 0
                offsets.push(ChangesetOffset { offset: 0, num_changes: 1 });
            }
        }
    }

    /// Removes `num` elements from end of tx or block range.
    pub fn prune(&mut self, num: u64) {
        if self.segment.is_block_based() {
            if let Some(range) = &mut self.block_range {
                if num > range.end - range.start {
                    self.block_range = None;
                    // Clear all changeset offsets if we're clearing all blocks
                    if self.segment.is_account_changesets() {
                        self.changeset_offsets = None;
                    }
                } else {
                    let old_end = range.end;
                    range.end = range.end.saturating_sub(num);

                    // Update changeset offsets for account changesets
                    if self.segment.is_account_changesets() &&
                        let Some(offsets) = &mut self.changeset_offsets
                    {
                        // Calculate how many blocks we're removing
                        let blocks_to_remove = old_end - range.end;
                        // Remove the last `blocks_to_remove` entries from offsets
                        let new_len = offsets.len().saturating_sub(blocks_to_remove as usize);
                        offsets.truncate(new_len);

                        // If we removed all offsets, set to None
                        if offsets.is_empty() {
                            self.changeset_offsets = None;
                        }
                    }
                }
            };
        } else if let Some(range) = &mut self.tx_range {
            if num > range.end - range.start {
                self.tx_range = None;
            } else {
                range.end = range.end.saturating_sub(num);
            }
        }
    }

    /// Sets a new `block_range`.
    #[allow(clippy::missing_const_for_fn)] // False positive: function mutates self
    pub fn set_block_range(&mut self, block_start: BlockNumber, block_end: BlockNumber) {
        if let Some(block_range) = &mut self.block_range {
            block_range.start = block_start;
            block_range.end = block_end;
        } else {
            self.block_range = Some(SegmentRangeInclusive::new(block_start, block_end))
        }
    }

    /// Synchronizes changeset offsets with the current block range for account changeset segments.
    ///
    /// This should be called after modifying the block range when dealing with changeset segments
    /// to ensure the offsets vector matches the block range size.
    pub fn sync_changeset_offsets(&mut self) {
        if !self.segment.is_account_changesets() {
            return;
        }

        if let Some(block_range) = &self.block_range {
            if let Some(offsets) = &mut self.changeset_offsets {
                let expected_len = (block_range.end - block_range.start + 1) as usize;
                if offsets.len() > expected_len {
                    offsets.truncate(expected_len);
                    if offsets.is_empty() {
                        self.changeset_offsets = None;
                    }
                }
            }
        } else {
            // No block range means no offsets
            self.changeset_offsets = None;
        }
    }

    /// Sets a new `tx_range`.
    pub const fn set_tx_range(&mut self, tx_start: TxNumber, tx_end: TxNumber) {
        if let Some(tx_range) = &mut self.tx_range {
            tx_range.start = tx_start;
            tx_range.end = tx_end;
        } else {
            self.tx_range = Some(SegmentRangeInclusive::new(tx_start, tx_end))
        }
    }

    /// Returns the row offset which depends on whether the segment is block or transaction based.
    pub fn start(&self) -> Option<u64> {
        if self.segment.is_block_based() {
            return self.block_start()
        }
        self.tx_start()
    }

    /// Returns the `ChangesetOffset` corresponding for the given block, if it's in the block
    /// range.
    ///
    /// If it is not in the block range or the changeset list in the header does not contain a
    /// value for the block, this returns `None`.
    pub fn changeset_offset(&self, block: BlockNumber) -> Option<&ChangesetOffset> {
        let block_range = self.block_range()?;
        if !(block_range.start()..=block_range.end()).contains(&block) {
            return None
        }

        let offsets = self.changeset_offsets.as_ref()?;
        let index = (block - block_range.start()) as usize;

        offsets.get(index)
    }
}

/// Configuration used on the segment.
#[derive(Debug, Clone, Copy)]
pub struct SegmentConfig {
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
    pub const fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// Start of the inclusive range
    pub const fn start(&self) -> u64 {
        self.start
    }

    /// End of the inclusive range
    pub const fn end(&self) -> u64 {
        self.end
    }
}

impl core::fmt::Display for SegmentRangeInclusive {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}..={}", self.start, self.end)
    }
}

impl From<RangeInclusive<u64>> for SegmentRangeInclusive {
    fn from(value: RangeInclusive<u64>) -> Self {
        Self { start: *value.start(), end: *value.end() }
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
    use alloy_primitives::hex;
    use reth_nippy_jar::NippyJar;

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
                "static_file_headers_2_30_none_lz4",
                Some(Compression::Lz4),
            ),
            (
                StaticFileSegment::Headers,
                2..=30,
                "static_file_headers_2_30_none_zstd",
                Some(Compression::Zstd),
            ),
            (
                StaticFileSegment::Headers,
                2..=30,
                "static_file_headers_2_30_none_zstd-dict",
                Some(Compression::ZstdWithDictionary),
            ),
        ];

        for (segment, block_range, filename, compression) in test_vectors {
            let block_range: SegmentRangeInclusive = block_range.into();
            if let Some(compression) = compression {
                assert_eq!(
                    segment.filename_with_configuration(compression, &block_range),
                    filename
                );
            } else {
                assert_eq!(segment.filename(&block_range), filename);
            }

            assert_eq!(StaticFileSegment::parse_filename(filename), Some((segment, block_range)));
        }

        assert_eq!(StaticFileSegment::parse_filename("static_file_headers_2"), None);
        assert_eq!(StaticFileSegment::parse_filename("static_file_headers_"), None);

        // roundtrip test
        let dummy_range = SegmentRangeInclusive::new(123, 1230);
        for segment in StaticFileSegment::iter() {
            let filename = segment.filename(&dummy_range);
            assert_eq!(Some((segment, dummy_range)), StaticFileSegment::parse_filename(&filename));
        }
    }

    #[test]
    fn test_segment_config_backwards() {
        let headers = hex!(
            "010000000000000000000000000000001fa10700000000000100000000000000001fa10700000000000000000000030000000000000020a107000000000001010000004a02000000000000"
        );
        let transactions = hex!(
            "010000000000000000000000000000001fa10700000000000100000000000000001fa107000000000001000000000000000034a107000000000001000000010000000000000035a1070000000000004010000000000000"
        );
        let receipts = hex!(
            "010000000000000000000000000000001fa10700000000000100000000000000000000000000000000000200000001000000000000000000000000000000000000000000000000"
        );

        {
            let headers = NippyJar::<SegmentHeader>::load_from_reader(&headers[..]).unwrap();
            assert_eq!(
                &SegmentHeader {
                    expected_block_range: SegmentRangeInclusive::new(0, 499999),
                    block_range: Some(SegmentRangeInclusive::new(0, 499999)),
                    tx_range: None,
                    segment: StaticFileSegment::Headers,
                    changeset_offsets: None,
                },
                headers.user_header()
            );
        }
        {
            let transactions =
                NippyJar::<SegmentHeader>::load_from_reader(&transactions[..]).unwrap();
            assert_eq!(
                &SegmentHeader {
                    expected_block_range: SegmentRangeInclusive::new(0, 499999),
                    block_range: Some(SegmentRangeInclusive::new(0, 499999)),
                    tx_range: Some(SegmentRangeInclusive::new(0, 500020)),
                    segment: StaticFileSegment::Transactions,
                    changeset_offsets: None,
                },
                transactions.user_header()
            );
        }
        {
            let receipts = NippyJar::<SegmentHeader>::load_from_reader(&receipts[..]).unwrap();
            assert_eq!(
                &SegmentHeader {
                    expected_block_range: SegmentRangeInclusive::new(0, 499999),
                    block_range: Some(SegmentRangeInclusive::new(0, 0)),
                    tx_range: None,
                    segment: StaticFileSegment::Receipts,
                    changeset_offsets: None,
                },
                receipts.user_header()
            );
        }
    }
}
