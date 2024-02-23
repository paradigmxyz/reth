//! Snapshot primitives.

mod compression;
mod filters;
mod segment;

use alloy_primitives::BlockNumber;
pub use compression::Compression;
pub use filters::{Filters, InclusionFilter, PerfectHashingFunction};
pub use segment::{SegmentConfig, SegmentHeader, SegmentRangeInclusive, StaticFileSegment};

/// Default snapshot block count.
pub const BLOCKS_PER_STATIC_FILE: u64 = 500_000;

/// Highest snapshotted block numbers, per data part.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct HighestStaticFiles {
    /// Highest snapshotted block of headers, inclusive.
    /// If [`None`], no snapshot is available.
    pub headers: Option<BlockNumber>,
    /// Highest snapshotted block of receipts, inclusive.
    /// If [`None`], no snapshot is available.
    pub receipts: Option<BlockNumber>,
    /// Highest snapshotted block of transactions, inclusive.
    /// If [`None`], no snapshot is available.
    pub transactions: Option<BlockNumber>,
}

impl HighestStaticFiles {
    /// Returns the highest snapshot if it exists for a segment
    pub fn highest(&self, segment: StaticFileSegment) -> Option<BlockNumber> {
        match segment {
            StaticFileSegment::Headers => self.headers,
            StaticFileSegment::Transactions => self.transactions,
            StaticFileSegment::Receipts => self.receipts,
        }
    }

    /// Returns a mutable reference to a snapshot segment
    pub fn as_mut(&mut self, segment: StaticFileSegment) -> &mut Option<BlockNumber> {
        match segment {
            StaticFileSegment::Headers => &mut self.headers,
            StaticFileSegment::Transactions => &mut self.transactions,
            StaticFileSegment::Receipts => &mut self.receipts,
        }
    }
}

/// Each snapshot has a fixed number of blocks. This gives out the range where the requested block
/// is positioned. Used for segment filename.
pub fn find_fixed_range(block: BlockNumber) -> SegmentRangeInclusive {
    let start = (block / BLOCKS_PER_STATIC_FILE) * BLOCKS_PER_STATIC_FILE;
    SegmentRangeInclusive::new(start, start + BLOCKS_PER_STATIC_FILE - 1)
}
