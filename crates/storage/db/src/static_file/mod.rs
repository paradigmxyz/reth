//! reth's static file database table import and access

use std::{collections::HashMap, path::Path};

mod cursor;
pub use cursor::StaticFileCursor;

mod mask;
pub use mask::*;
use reth_nippy_jar::{NippyJar, NippyJarError};

mod masks;
pub use masks::*;
use reth_static_file_types::{SegmentHeader, SegmentRangeInclusive, StaticFileSegment};

/// Alias type for a map of [`StaticFileSegment`] and sorted lists of existing static file ranges.
type SortedStaticFiles = HashMap<StaticFileSegment, Vec<(SegmentRangeInclusive, SegmentHeader)>>;

/// Given the `static_files` directory path, it returns a list over the existing `static_files`
/// organized by [`StaticFileSegment`]. Each segment has a sorted list of block ranges and
/// segment headers as presented in the file configuration.
pub fn iter_static_files(path: &Path) -> Result<SortedStaticFiles, NippyJarError> {
    if !path.exists() {
        reth_fs_util::create_dir_all(path).map_err(|err| NippyJarError::Custom(err.to_string()))?;
    }

    let mut static_files = SortedStaticFiles::default();
    let entries = reth_fs_util::read_dir(path)
        .map_err(|err| NippyJarError::Custom(err.to_string()))?
        .filter_map(Result::ok);
    for entry in entries {
        if entry.metadata().is_ok_and(|metadata| metadata.is_file()) &&
            let Some((segment, _)) =
                StaticFileSegment::parse_filename(&entry.file_name().to_string_lossy())
        {
            let jar = NippyJar::<SegmentHeader>::load(&entry.path())?;

            if let Some(block_range) = jar.user_header().block_range() {
                static_files
                    .entry(segment)
                    .and_modify(|headers| headers.push((block_range, jar.user_header().clone())))
                    .or_insert_with(|| vec![(block_range, jar.user_header().clone())]);
            }
        }
    }

    for range_list in static_files.values_mut() {
        // Sort by block end range.
        range_list.sort_by_key(|(block_range, _)| block_range.end());
    }

    Ok(static_files)
}
