//! reth's snapshot database table import and access

mod generation;
use std::{
    collections::{hash_map::Entry, HashMap},
    ops::RangeInclusive,
    path::Path,
};

pub use generation::*;

mod cursor;
pub use cursor::SnapshotCursor;

mod mask;
pub use mask::*;
use reth_nippy_jar::{NippyJar, NippyJarError};
use reth_primitives::{snapshot::SegmentHeader, BlockNumber, SnapshotSegment, TxNumber};

mod masks;

/// Alias type for a map of [`SnapshotSegment`] and sorted lists of existing snapshot ranges.
type SortedSnapshots =
    HashMap<SnapshotSegment, Vec<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>>;

/// Given the snapshots directory path, it returns a list over the existing snapshots organized by
/// [`SnapshotSegment`]. Each segment has a sorted list of block ranges and transaction ranges.
pub fn iter_snapshots(path: impl AsRef<Path>) -> Result<SortedSnapshots, NippyJarError> {
    let mut static_files = SortedSnapshots::default();
    let entries = reth_primitives::fs::read_dir(path.as_ref())
        .map_err(|err| NippyJarError::Custom(err.to_string()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();

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

    for (segment, range_list) in static_files.iter_mut() {
        // Sort by block end range.
        range_list.sort_by(|a, b| a.0.end().cmp(b.0.end()));

        if let Some((block_range, tx_range)) = range_list.pop() {
            // The highest height static file filename might not be indicative of its actual
            // block_range, so we need to read its actual configuration.
            let jar = NippyJar::<SegmentHeader>::load(
                &path.as_ref().join(segment.filename(&block_range, &tx_range)),
            )?;

            if &tx_range != jar.user_header().tx_range() {
                // TODO(joshie): rename
            }

            range_list.push((
                jar.user_header().block_range().clone(),
                jar.user_header().tx_range().clone(),
            ));
        }
    }

    Ok(static_files)
}
