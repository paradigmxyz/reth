//! reth's static file database table import and access

use reth_nippy_jar::{NippyJar, NippyJarError, CONFIG_FILE_EXTENSION};
use reth_static_file_types::{
    SegmentHeader, SegmentRangeInclusive, StaticFileMap, StaticFileSegment,
};
use std::path::Path;
use tracing::warn;

mod cursor;
pub use cursor::StaticFileCursor;

mod mask;
pub use mask::*;

mod masks;
pub use masks::*;

/// Alias type for a map of [`StaticFileSegment`] and sorted lists of existing static file ranges.
type SortedStaticFiles = StaticFileMap<Vec<(SegmentRangeInclusive, SegmentHeader)>>;

/// Given the `static_files` directory path, it returns a list over the existing `static_files`
/// organized by [`StaticFileSegment`]. Each segment has a sorted list of block ranges and
/// segment headers as presented in the file configuration.
pub fn iter_static_files(path: &Path) -> Result<SortedStaticFiles, NippyJarError> {
    let mut static_files = SortedStaticFiles::default();
    visit_static_file_headers(path, |segment, _, header, _| {
        if let Some(range) = header.block_range() {
            static_files.entry(segment).or_default().push((range, header.clone()));
        }
    })?;

    // Sort by block end range.
    for range_list in static_files.values_mut() {
        range_list.sort_unstable_by_key(|(block_range, _)| block_range.end());
    }

    Ok(static_files)
}

/// Visits initialized jars in directory order, including jars with no committed block range.
/// Supplies the segment and fixed range from the filename, the user header, and the committed
/// row count. Returns an error if opening the directory or loading a jar configuration fails.
pub fn visit_static_file_headers(
    path: &Path,
    mut visit: impl FnMut(StaticFileSegment, SegmentRangeInclusive, &SegmentHeader, usize),
) -> Result<(), NippyJarError> {
    if !path.exists() {
        reth_fs_util::create_dir_all(path).map_err(|err| NippyJarError::Custom(err.to_string()))?;
    }

    let entries = reth_fs_util::read_dir(path)
        .map_err(|err| NippyJarError::Custom(err.to_string()))?
        .filter_map(Result::ok);
    for entry in entries {
        if entry.metadata().is_ok_and(|metadata| metadata.is_file()) &&
            let Some((segment, fixed_range)) =
                StaticFileSegment::parse_filename(&entry.file_name().to_string_lossy())
        {
            let data_path = entry.path();
            // The config is written last on creation; without it the jar was never initialized
            // and the writer recreates it.
            if !data_path.with_extension(CONFIG_FILE_EXTENSION).exists() {
                warn!(
                    target: "reth::static_file",
                    path = ?data_path,
                    "Skipping uninitialized static file"
                );
                continue
            }
            let jar = NippyJar::<SegmentHeader>::load(&data_path)?;

            visit(segment, fixed_range, jar.user_header(), jar.rows());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_static_file_without_config() {
        let dir = tempfile::tempdir().unwrap();
        let segment = StaticFileSegment::Headers;
        let path = dir.path().join(segment.filename(&SegmentRangeInclusive::new(0, 499_999)));
        std::fs::write(&path, b"").unwrap();

        assert!(iter_static_files(dir.path()).unwrap().is_empty());
    }
}
