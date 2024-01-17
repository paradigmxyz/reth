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
    HashMap<SnapshotSegment, Vec<(RangeInclusive<BlockNumber>, Option<RangeInclusive<TxNumber>>)>>;

/// Given the snapshots directory path, it returns a list over the existing snapshots organized by
/// [`SnapshotSegment`]. Each segment has a sorted list of block ranges and transaction ranges as
/// presented in the file configuration.
pub fn iter_snapshots(path: impl AsRef<Path>) -> Result<SortedSnapshots, NippyJarError> {
    let path = path.as_ref();
    if !path.exists() {
        reth_primitives::fs::create_dir_all(path)
            .map_err(|err| NippyJarError::Custom(err.to_string()))?;
    }

    let mut static_files = SortedSnapshots::default();
    let entries = reth_primitives::fs::read_dir(path)
        .map_err(|err| NippyJarError::Custom(err.to_string()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();

    for entry in entries {
        if entry.metadata().map_or(false, |metadata| metadata.is_file()) {
            if let Some((segment, _)) = SnapshotSegment::parse_filename(&entry.file_name()) {
                let jar = NippyJar::<SegmentHeader>::load(&entry.path())?;

                let ranges =
                    (jar.user_header().block_range().clone(), jar.user_header().tx_range());

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

    for (_, range_list) in static_files.iter_mut() {
        // Sort by block end range.
        range_list.sort_by(|a, b| a.0.end().cmp(b.0.end()));
    }

    Ok(static_files)
}
