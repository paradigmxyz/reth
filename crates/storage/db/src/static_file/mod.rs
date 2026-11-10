//! reth's static file database table import and access

use reth_nippy_jar::{NippyJar, NippyJarError};
use reth_static_file_types::{
    SegmentHeader, SegmentRangeInclusive, StaticFileMap, StaticFileSegment,
};
use std::{io::ErrorKind, path::Path};

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
/// segment headers as presented in the file configuration. Empty bare data files without any
/// companion files are ignored without modifying the directory.
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
            let Some((segment, expected_range)) =
                StaticFileSegment::parse_filename(&entry.file_name().to_string_lossy())
        {
            let data_path = entry.path();
            if entry.file_name() == segment.filename(&expected_range).as_str() &&
                entry.file_type()?.is_file() &&
                entry.metadata()?.len() == 0
            {
                // Only bare data files can be ignored without discarding companion metadata.
                let mut bare = true;
                for extension in ["conf", "off", "idx", "csoff"] {
                    let companion = data_path.with_extension(extension);
                    match std::fs::symlink_metadata(&companion) {
                        Ok(_) => {
                            bare = false;
                            break
                        }
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(reth_fs_util::FsPathError::metadata(error, companion).into())
                        }
                    }
                }
                if bare {
                    continue;
                }
            }
            let jar = NippyJar::<SegmentHeader>::load(&data_path)?;

            if let Some(block_range) = jar.user_header().block_range() {
                static_files
                    .entry(segment)
                    .and_modify(|headers| headers.push((block_range, jar.user_header().clone())))
                    .or_insert_with(|| vec![(block_range, jar.user_header().clone())]);
            }
        }
    }

    // Sort by block end range.
    for range_list in static_files.values_mut() {
        range_list.sort_unstable_by_key(|(block_range, _)| block_range.end());
    }

    Ok(static_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_nippy_jar::NippyJarWriter;
    use std::fs;

    #[test]
    fn ignores_only_bare_empty_data() {
        for segment in StaticFileSegment::iter() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(segment.filename(&SegmentRangeInclusive::new(0, 499_999)));
            fs::write(&path, []).unwrap();

            assert!(iter_static_files(dir.path()).unwrap().is_empty());
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            assert_eq!(fs::read(&path).unwrap(), Vec::<u8>::new());
        }
    }

    #[test]
    fn rejects_missing_config_with_data_or_companions() {
        for (data, extension) in [
            (&b"data"[..], None),
            (&b""[..], Some("off")),
            (&b""[..], Some("idx")),
            (&b""[..], Some("csoff")),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("static_file_transaction-senders_0_499999");
            fs::write(&path, data).unwrap();
            if let Some(extension) = extension {
                fs::write(path.with_extension(extension), [8, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
            }

            let error = iter_static_files(dir.path()).unwrap_err();
            assert!(matches!(error, NippyJarError::FileSystem(reth_fs_util::FsPathError::Open {
                source, path: config
            }) if source.kind() == ErrorKind::NotFound && config == path.with_extension("conf")));
            assert_eq!(fs::read(&path).unwrap(), data);
            assert_eq!(
                fs::read_dir(dir.path()).unwrap().count(),
                1 + usize::from(extension.is_some())
            );
            if let Some(extension) = extension {
                assert_eq!(
                    fs::read(path.with_extension(extension)).unwrap(),
                    [8, 0, 0, 0, 0, 0, 0, 0, 0]
                );
            }
        }
    }

    #[test]
    fn rejects_invalid_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("static_file_transaction-senders_0_499999");
        fs::write(&path, []).unwrap();
        fs::write(path.with_extension("conf"), []).unwrap();

        assert!(matches!(iter_static_files(dir.path()).unwrap_err(), NippyJarError::Bincode(_)));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        assert_eq!(fs::read(path.with_extension("conf")).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn preserves_zero_row_committed_block_range() {
        let dir = tempfile::tempdir().unwrap();
        let expected_range = SegmentRangeInclusive::new(0, 499_999);
        let block_range = SegmentRangeInclusive::new(0, 0);
        let segment = StaticFileSegment::TransactionSenders;
        let path = dir.path().join(segment.filename(&expected_range));
        let header = SegmentHeader::new(expected_range, Some(block_range), None, segment);
        let jar = NippyJar::new(segment.columns(), &path, header.clone());
        fs::write(&path, []).unwrap();
        jar.save_to_writer(fs::File::create(path.with_extension("conf")).unwrap()).unwrap();
        let config = fs::read(path.with_extension("conf")).unwrap();

        let files = iter_static_files(dir.path()).unwrap();
        assert_eq!(files.get(segment).unwrap(), &vec![(block_range, header)]);
        assert_eq!(fs::read(path.with_extension("conf")).unwrap(), config);
        assert_eq!(fs::read(&path).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn rejects_noncanonical_bare_data_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("static_file_transaction-senders_0_499999_extra");
        fs::write(&path, []).unwrap();

        assert!(matches!(iter_static_files(dir.path()).unwrap_err(),
            NippyJarError::FileSystem(reth_fs_util::FsPathError::Open { source, .. })
                if source.kind() == ErrorKind::NotFound));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_config_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("static_file_transaction-senders_0_499999");
        fs::write(&path, []).unwrap();
        std::os::unix::fs::symlink("missing", path.with_extension("conf")).unwrap();

        assert!(matches!(iter_static_files(dir.path()).unwrap_err(),
            NippyJarError::FileSystem(reth_fs_util::FsPathError::Open { source, .. })
                if source.kind() == ErrorKind::NotFound));
        assert_eq!(fs::read_link(path.with_extension("conf")).unwrap(), Path::new("missing"));
    }

    #[cfg(unix)]
    #[test]
    fn ignores_data_symlink_without_mutating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("static_file_transaction-senders_0_499999");
        let target = dir.path().join("empty");
        fs::write(&target, []).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        assert!(iter_static_files(dir.path()).unwrap().is_empty());
        assert_eq!(fs::read_link(&path).unwrap(), target);
    }

    #[test]
    fn bare_data_can_be_initialized_and_rediscovered() {
        let dir = tempfile::tempdir().unwrap();
        let expected_range = SegmentRangeInclusive::new(0, 499_999);
        let block_range = SegmentRangeInclusive::new(0, 0);
        let segment = StaticFileSegment::TransactionSenders;
        let path = dir.path().join(segment.filename(&expected_range));
        fs::write(&path, []).unwrap();
        assert!(iter_static_files(dir.path()).unwrap().is_empty());

        let header = SegmentHeader::new(expected_range, None, None, segment);
        let mut writer =
            NippyJarWriter::new(NippyJar::new(segment.columns(), &path, header)).unwrap();
        writer.append_column(Some(Ok(b"sender"))).unwrap();
        let committed_header = SegmentHeader::new(
            expected_range,
            Some(block_range),
            Some(SegmentRangeInclusive::new(0, 0)),
            segment,
        );
        *writer.user_header_mut() = committed_header.clone();
        writer.commit().unwrap();
        drop(writer);

        let files = iter_static_files(dir.path()).unwrap();
        assert_eq!(files.get(segment).unwrap(), &vec![(block_range, committed_header)]);
        let jar = NippyJar::<SegmentHeader>::load(&path).unwrap();
        assert_eq!(jar.rows(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"sender");
    }
}
