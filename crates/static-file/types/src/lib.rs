//! Commonly used types for static file usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod compression;
mod segment;

use alloy_primitives::BlockNumber;
pub use compression::Compression;
pub use segment::{SegmentConfig, SegmentHeader, SegmentRangeInclusive, StaticFileSegment};

/// Default static file block count.
pub const DEFAULT_BLOCKS_PER_STATIC_FILE: u64 = 500_000;

/// Highest static file block numbers, per data segment.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct HighestStaticFiles {
    /// Highest static file block of headers, inclusive.
    /// If [`None`], no static file is available.
    pub headers: Option<BlockNumber>,
    /// Highest static file block of receipts, inclusive.
    /// If [`None`], no static file is available.
    pub receipts: Option<BlockNumber>,
    /// Highest static file block of transactions, inclusive.
    /// If [`None`], no static file is available.
    pub transactions: Option<BlockNumber>,
}

impl HighestStaticFiles {
    /// Returns the highest static file if it exists for a segment
    pub const fn highest(&self, segment: StaticFileSegment) -> Option<BlockNumber> {
        match segment {
            StaticFileSegment::Headers => self.headers,
            StaticFileSegment::Transactions => self.transactions,
            StaticFileSegment::Receipts => self.receipts,
        }
    }

    /// Returns a mutable reference to a static file segment
    pub fn as_mut(&mut self, segment: StaticFileSegment) -> &mut Option<BlockNumber> {
        match segment {
            StaticFileSegment::Headers => &mut self.headers,
            StaticFileSegment::Transactions => &mut self.transactions,
            StaticFileSegment::Receipts => &mut self.receipts,
        }
    }

    /// Returns the minimum block of all segments.
    pub fn min(&self) -> Option<u64> {
        [self.headers, self.transactions, self.receipts].iter().filter_map(|&option| option).min()
    }

    /// Returns the maximum block of all segments.
    pub fn max(&self) -> Option<u64> {
        [self.headers, self.transactions, self.receipts].iter().filter_map(|&option| option).max()
    }
}

/// Each static file has a fixed number of blocks. This gives out the range where the requested
/// block is positioned. Used for segment filename.
pub const fn find_fixed_range(
    block: BlockNumber,
    blocks_per_static_file: u64,
) -> SegmentRangeInclusive {
    let start = (block / blocks_per_static_file) * blocks_per_static_file;
    SegmentRangeInclusive::new(start, start + blocks_per_static_file - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highest_static_files_highest() {
        let files =
            HighestStaticFiles { headers: Some(100), receipts: Some(200), transactions: None };

        // Test for headers segment
        assert_eq!(files.highest(StaticFileSegment::Headers), Some(100));

        // Test for receipts segment
        assert_eq!(files.highest(StaticFileSegment::Receipts), Some(200));

        // Test for transactions segment
        assert_eq!(files.highest(StaticFileSegment::Transactions), None);
    }

    #[test]
    fn test_highest_static_files_as_mut() {
        let mut files = HighestStaticFiles::default();

        // Modify headers value
        *files.as_mut(StaticFileSegment::Headers) = Some(150);
        assert_eq!(files.headers, Some(150));

        // Modify receipts value
        *files.as_mut(StaticFileSegment::Receipts) = Some(250);
        assert_eq!(files.receipts, Some(250));

        // Modify transactions value
        *files.as_mut(StaticFileSegment::Transactions) = Some(350);
        assert_eq!(files.transactions, Some(350));
    }

    #[test]
    fn test_highest_static_files_min() {
        let files =
            HighestStaticFiles { headers: Some(300), receipts: Some(100), transactions: None };

        // Minimum value among the available segments
        assert_eq!(files.min(), Some(100));

        let empty_files = HighestStaticFiles::default();
        // No values, should return None
        assert_eq!(empty_files.min(), None);
    }

    #[test]
    fn test_highest_static_files_max() {
        let files =
            HighestStaticFiles { headers: Some(300), receipts: Some(100), transactions: Some(500) };

        // Maximum value among the available segments
        assert_eq!(files.max(), Some(500));

        let empty_files = HighestStaticFiles::default();
        // No values, should return None
        assert_eq!(empty_files.max(), None);
    }

    #[test]
    fn test_find_fixed_range() {
        // Test with default block size
        let block: BlockNumber = 600_000;
        let range = find_fixed_range(block, DEFAULT_BLOCKS_PER_STATIC_FILE);
        assert_eq!(range.start(), 500_000);
        assert_eq!(range.end(), 999_999);

        // Test with a custom block size
        let block: BlockNumber = 1_200_000;
        let range = find_fixed_range(block, 1_000_000);
        assert_eq!(range.start(), 1_000_000);
        assert_eq!(range.end(), 1_999_999);
    }
}
