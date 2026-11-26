//! reth's static file database table import and access

use reth_nippy_jar::{NippyJar, NippyJarError};
use reth_static_file_types::{
    SegmentHeader, SegmentRangeInclusive, StaticFileMap, StaticFileSegment,
};
use std::path::Path;
use tracing::warn;

pub use reth_nippy_jar::{CONFIG_FILE_EXTENSION, OFFSETS_FILE_EXTENSION};

mod cursor;
pub use cursor::StaticFileCursor;

mod mask;
pub use mask::*;

mod masks;
pub use masks::*;

/// A static file found on disk.
#[derive(Debug, Clone)]
pub struct StaticFileEntry {
    /// The block range the file is named after, which its segment index keys it by.
    pub file_range: SegmentRangeInclusive,
    /// The blocks the file holds, within `file_range`.
    pub block_range: SegmentRangeInclusive,
    /// The file's header.
    pub header: SegmentHeader,
}

/// Alias type for a map of [`StaticFileSegment`] and lists of existing static files, sorted by
/// the end of the blocks they hold.
type SortedStaticFiles = StaticFileMap<Vec<StaticFileEntry>>;

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
            let Some((segment, file_range)) =
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
            let header = read_static_file_header(&data_path)?;

            if let Some(block_range) = header.block_range() {
                static_files.entry(segment).or_insert_with(Vec::new).push(StaticFileEntry {
                    file_range,
                    block_range,
                    header,
                });
            }
        }
    }

    // Sort by block end range.
    for range_list in static_files.values_mut() {
        range_list.sort_unstable_by_key(|entry| entry.block_range.end());
    }

    Ok(static_files)
}

/// Reads the segment header from the configuration of a single static file.
pub fn read_static_file_header(path: &Path) -> Result<SegmentHeader, NippyJarError> {
    Ok(NippyJar::<SegmentHeader>::load(path)?.user_header().clone())
}

/// Returns the last missing block at or below `tip`, or `None` if coverage reaches genesis.
/// Ranges must be ordered by decreasing end block. Overlapping ranges are allowed.
pub fn last_missing_block(
    tip: u64,
    ranges: impl Iterator<Item = SegmentRangeInclusive>,
) -> Option<u64> {
    let mut missing = tip;
    for range in ranges {
        if range.end() < missing {
            break
        }
        if range.contains(missing) {
            missing = range.start().checked_sub(1)?;
        }
    }
    Some(missing)
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
