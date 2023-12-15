//! Snapshot primitives.

mod compression;
mod filters;
mod segment;

use alloy_primitives::{BlockNumber, TxNumber};
pub use compression::Compression;
pub use filters::{Filters, InclusionFilter, PerfectHashingFunction};
pub use segment::{SegmentConfig, SegmentHeader, SnapshotSegment};

use crate::fs::FsPathError;
use std::{
    collections::{hash_map::Entry, HashMap},
    ops::RangeInclusive,
    path::Path,
};

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

/// Alias type for a map of [`SnapshotSegment`] and sorted lists of existing snapshot ranges.
type SortedSnapshots =
    HashMap<SnapshotSegment, Vec<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>>;

/// Given the snapshot's location, it returns a list over the existing snapshots organized by
/// [`SnapshotSegment`]. Each segment has a sorted list of block ranges and transaction ranges.
pub fn iter_snapshots(path: impl AsRef<Path>) -> Result<SortedSnapshots, FsPathError> {
    let mut static_files: HashMap<SnapshotSegment, Vec<_>> = HashMap::default();
    let entries = crate::fs::read_dir(path.as_ref())?.filter_map(Result::ok).collect::<Vec<_>>();

    for entry in entries {
        if entry.metadata().map_or(false, |metadata| metadata.is_file()) {
            if let Some((segment, block_range, tx_range)) =
                SnapshotSegment::parse_filename(&entry.file_name())
            {
                let ranges = (block_range, tx_range);
                match static_files.entry(segment) {
                    Entry::Occupied(mut entry) => {
                        entry.get_mut().push(ranges);
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(vec![ranges]);
                    }
                }
            }
        }
    }

    // Sort by block end range.
    for (_, range_list) in static_files.iter_mut() {
        range_list.sort_by(|a, b| a.0.end().cmp(b.0.end()));
    }

    Ok(static_files)
}
