//! Snapshot primitives.

mod compression;
mod filters;
mod segment;

use alloy_primitives::{BlockNumber, TxNumber};
pub use compression::Compression;
pub use filters::{Filters, InclusionFilter, PerfectHashingFunction};
pub use segment::{SegmentConfig, SegmentHeader, SnapshotSegment};

use crate::fs::FsPathError;
use std::{ops::RangeInclusive, path::Path};

/// Default snapshot block count.
pub const BLOCKS_PER_SNAPSHOT: u64 = 500_000;

/// Highest snapshotted block numbers, per data part.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct HighestSnapshots {
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

impl HighestSnapshots {
    /// Returns the highest snapshot if it exists for a segment
    pub fn highest(&self, segment: SnapshotSegment) -> Option<BlockNumber> {
        match segment {
            SnapshotSegment::Headers => self.headers,
            SnapshotSegment::Transactions => self.transactions,
            SnapshotSegment::Receipts => self.receipts,
        }
    }

    /// Returns a mutable reference to a snapshot segment
    pub fn as_mut(&mut self, segment: SnapshotSegment) -> &mut Option<BlockNumber> {
        match segment {
            SnapshotSegment::Headers => &mut self.headers,
            SnapshotSegment::Transactions => &mut self.transactions,
            SnapshotSegment::Receipts => &mut self.receipts,
        }
    }
}

/// Given the snapshot's location, it returns an iterator over the existing snapshots in the format
/// of a tuple composed by the segment, block range and transaction range.
pub fn iter_snapshots(
    path: impl AsRef<Path>,
) -> Result<
    impl Iterator<Item = (SnapshotSegment, RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>,
    FsPathError,
> {
    let entries = crate::fs::read_dir(path.as_ref())?.filter_map(Result::ok);
    Ok(entries.filter_map(|entry| {
        if entry.metadata().map_or(false, |metadata| metadata.is_file()) {
            return SnapshotSegment::parse_filename(&entry.file_name())
        }
        None
    }))
}
