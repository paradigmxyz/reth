//! reth's static file database table import and access

mod generation;
use std::{
    collections::{hash_map::Entry, HashMap},
    path::Path,
};

pub use generation::*;

mod cursor;
pub use cursor::StaticFileCursor;

mod mask;
pub use mask::*;
use reth_nippy_jar::{NippyJar, NippyJarError};
use reth_primitives::{
    static_file::{SegmentHeader, SegmentRangeInclusive},
    StaticFileSegment,
};

mod masks;

/// Alias type for a map of [`StaticFileSegment`] and sorted lists of existing static file ranges.
type SortedStaticFiles =
    HashMap<StaticFileSegment, Vec<(SegmentRangeInclusive, Option<SegmentRangeInclusive>)>>;

/// Given the static_files directory path, it returns a list over the existing static_files
/// organized by [`StaticFileSegment`]. Each segment has a sorted list of block ranges and
/// transaction ranges as presented in the file configuration.
pub fn iter_static_files(path: impl AsRef<Path>) -> Result<SortedStaticFiles, NippyJarError> {
    let path = path.as_ref();
    if !path.exists() {
        reth_primitives::fs::create_dir_all(path)
            .map_err(|err| NippyJarError::Custom(err.to_string()))?;
    }

    let mut static_files = SortedStaticFiles::default();
    let entries = reth_primitives::fs::read_dir(path)
        .map_err(|err| NippyJarError::Custom(err.to_string()))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();

    for entry in entries {
        if entry.metadata().map_or(false, |metadata| metadata.is_file()) {
            if let Some((segment, _)) =
                StaticFileSegment::parse_filename(&entry.file_name().to_string_lossy())
            {
                let jar = NippyJar::<SegmentHeader>::load(&entry.path())?;

                let (block_range, tx_range) = (
                    jar.user_header().block_range().copied(),
                    jar.user_header().tx_range().copied(),
                );

                if let Some(block_range) = block_range {
                    match static_files.entry(segment) {
                        Entry::Occupied(mut entry) => {
                            entry.get_mut().push((block_range, tx_range));
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(vec![(block_range, tx_range)]);
                        }
                    }
                }
            }
        }
    }

    for (_, range_list) in static_files.iter_mut() {
        // Sort by block end range.
        range_list.sort_by(|a, b| a.0.end().cmp(&b.0.end()));
    }

    Ok(static_files)
}
