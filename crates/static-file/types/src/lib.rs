//! Commonly used types for static file usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod compression;
mod event;
mod segment;

#[cfg(feature = "std")]
mod changeset_offsets;
#[cfg(feature = "std")]
pub use changeset_offsets::{ChangesetOffsetReader, ChangesetOffsetWriter};

use alloy_primitives::BlockNumber;
pub use compression::Compression;
use core::ops::RangeInclusive;
pub use event::StaticFileProducerEvent;
pub use segment::{
    ChangesetOffset, SegmentConfig, SegmentHeader, SegmentRangeInclusive, StaticFileSegment,
};

/// Map keyed by [`StaticFileSegment`].
pub type StaticFileMap<T> = alloc::boxed::Box<fixed_map::Map<StaticFileSegment, T>>;

/// Default static file block count.
pub const DEFAULT_BLOCKS_PER_STATIC_FILE: u64 = 500_000;

/// Highest static file block numbers, per data segment.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct HighestStaticFiles {
    /// Highest static file block of receipts, inclusive.
    /// If [`None`], no static file is available.
    pub receipts: Option<BlockNumber>,
}

impl HighestStaticFiles {
    /// Returns an iterator over all static file segments
    fn iter(&self) -> impl Iterator<Item = Option<BlockNumber>> {
        [self.receipts].into_iter()
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
    /// Targeted range of receipts.
    pub receipts: Option<RangeInclusive<BlockNumber>>,
}

impl StaticFileTargets {
    /// Returns `true` if any of the targets are [Some].
    pub const fn any(&self) -> bool {
        self.receipts.is_some()
    }

    /// Returns `true` if all targets are either [`None`] or has beginning of the range equal to the
    /// highest static file.
    pub fn is_contiguous_to_highest_static_files(&self, static_files: HighestStaticFiles) -> bool {
        core::iter::once(&(self.receipts.as_ref(), static_files.receipts)).all(
            |(target_block_range, highest_static_file_block)| {
                target_block_range.is_none_or(|target_block_range| {
                    *target_block_range.start() ==
                        highest_static_file_block
                            .map_or(0, |highest_static_file_block| highest_static_file_block + 1)
                })
            },
        )
    }
}

/// Each static file has a fixed number of blocks. This gives out the range where the requested
/// block is positioned, according to the specified number of blocks per static file.
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
    fn test_highest_static_files_min() {
        let files = HighestStaticFiles { receipts: Some(100) };

        // Minimum value among the available segments
        assert_eq!(files.min_block_num(), Some(100));

        let empty_files = HighestStaticFiles::default();
        // No values, should return None
        assert_eq!(empty_files.min_block_num(), None);
    }

    #[test]
    fn test_highest_static_files_max() {
        let files = HighestStaticFiles { receipts: Some(100) };

        // Maximum value among the available segments
        assert_eq!(files.max_block_num(), Some(100));

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
