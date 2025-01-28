//! Commonly used types for static file usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod compression;
mod event;
mod segment;

use alloy_primitives::BlockNumber;
pub use compression::Compression;
pub use event::StaticFileProducerEvent;
pub use segment::{SegmentConfig, SegmentHeader, SegmentRangeInclusive, StaticFileSegment};
use std::ops::RangeInclusive;

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
    /// Highest static file block of transactions, inclusive.
    /// If [`None`], no static file is available.
    pub block_meta: Option<BlockNumber>,
}

impl HighestStaticFiles {
    /// Returns the highest static file if it exists for a segment
    pub const fn highest(&self, segment: StaticFileSegment) -> Option<BlockNumber> {
        match segment {
            StaticFileSegment::Headers => self.headers,
            StaticFileSegment::Transactions => self.transactions,
            StaticFileSegment::Receipts => self.receipts,
            StaticFileSegment::BlockMeta => self.block_meta,
        }
    }

    /// Returns a mutable reference to a static file segment
    pub fn as_mut(&mut self, segment: StaticFileSegment) -> &mut Option<BlockNumber> {
        match segment {
            StaticFileSegment::Headers => &mut self.headers,
            StaticFileSegment::Transactions => &mut self.transactions,
            StaticFileSegment::Receipts => &mut self.receipts,
            StaticFileSegment::BlockMeta => &mut self.block_meta,
        }
    }

    /// Returns an iterator over all static file segments
    fn iter(&self) -> impl Iterator<Item = Option<BlockNumber>> {
        [self.headers, self.transactions, self.receipts, self.block_meta].into_iter()
    }

    /// Returns the minimum block of all segments.
    pub fn min_block_num(&self) -> Option<u64> {
        self.iter().flatten().min()
    }

    /// Returns the maximum block of all segments.
    pub fn max_block_num(&self) -> Option<u64> {
        self.iter().flatten().max()
    }
}

/// Static File targets, per data segment, measured in [`BlockNumber`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StaticFileTargets {
    /// Targeted range of headers.
    pub headers: Option<RangeInclusive<BlockNumber>>,
    /// Targeted range of receipts.
    pub receipts: Option<RangeInclusive<BlockNumber>>,
    /// Targeted range of transactions.
    pub transactions: Option<RangeInclusive<BlockNumber>>,
    /// Targeted range of block meta.
    pub block_meta: Option<RangeInclusive<BlockNumber>>,
}

impl StaticFileTargets {
    /// Returns `true` if any of the targets are [Some].
    pub const fn any(&self) -> bool {
        self.headers.is_some() ||
            self.receipts.is_some() ||
            self.transactions.is_some() ||
            self.block_meta.is_some()
    }

    /// Returns `true` if all targets are either [`None`] or has beginning of the range equal to the
    /// highest static file.
    pub fn is_contiguous_to_highest_static_files(&self, static_files: HighestStaticFiles) -> bool {
        [
            (self.headers.as_ref(), static_files.headers),
            (self.receipts.as_ref(), static_files.receipts),
            (self.transactions.as_ref(), static_files.transactions),
            (self.block_meta.as_ref(), static_files.block_meta),
        ]
        .iter()
        .all(|(target_block_range, highest_static_fileted_block)| {
            target_block_range.is_none_or(|target_block_range| {
                *target_block_range.start() ==
                    highest_static_fileted_block.map_or(0, |highest_static_fileted_block| {
                        highest_static_fileted_block + 1
                    })
            })
        })
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
        let files = HighestStaticFiles {
            headers: Some(100),
            receipts: Some(200),
            transactions: None,
            block_meta: None,
        };

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

        // Modify block meta value
        *files.as_mut(StaticFileSegment::BlockMeta) = Some(350);
        assert_eq!(files.block_meta, Some(350));
    }

    #[test]
    fn test_highest_static_files_min() {
        let files = HighestStaticFiles {
            headers: Some(300),
            receipts: Some(100),
            transactions: None,
            block_meta: None,
        };

        // Minimum value among the available segments
        assert_eq!(files.min_block_num(), Some(100));

        let empty_files = HighestStaticFiles::default();
        // No values, should return None
        assert_eq!(empty_files.min_block_num(), None);
    }

    #[test]
    fn test_highest_static_files_max() {
        let files = HighestStaticFiles {
            headers: Some(300),
            receipts: Some(100),
            transactions: Some(500),
            block_meta: Some(500),
        };

        // Maximum value among the available segments
        assert_eq!(files.max_block_num(), Some(500));

        let empty_files = HighestStaticFiles::default();
        // No values, should return None
        assert_eq!(empty_files.max_block_num(), None);
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
