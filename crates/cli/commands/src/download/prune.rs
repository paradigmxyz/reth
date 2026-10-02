//! Removes snapshot-managed data that the selected download plan does not list.

use super::{
    managed_datadir_path, planning::PlannedArchive, verify::OutputVerifier,
    FORCE_REMOVED_DATADIR_PATHS,
};
use eyre::{Result, WrapErr};
use reth_fs_util as fs;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tracing::info;

/// Removes every file under the paths that `--force` clears (`db`, `rocksdb`, `static_files`,
/// and `reth.toml`) that no planned archive lists as an output.
///
/// Listed files are kept so the startup integrity check can reuse the ones that still verify.
/// Once the download completes, those paths hold exactly the snapshot's files, with no leftovers
/// such as blocks or history indices from a chain the node extended past the snapshot block.
///
/// A managed path that is itself a symlink is followed, because operators link whole data
/// directories to other disks. Symlinks below it are removed rather than followed, so pruning
/// never deletes files outside the data dir. Nested directories left empty are removed; the
/// managed directories themselves are kept, since they may be mount points.
pub(crate) fn prune_unlisted_outputs(
    planned: &[PlannedArchive],
    target_dir: &Path,
    static_files_dir: Option<&Path>,
) -> Result<()> {
    let verifier = OutputVerifier::new(target_dir, static_files_dir);
    let listed = planned
        .iter()
        .flat_map(|planned| &planned.archive.output_files)
        .map(|output| verifier.output_path(&output.path))
        .collect::<HashSet<_>>();

    let mut removed = 0;
    for entry in FORCE_REMOVED_DATADIR_PATHS {
        let root = managed_datadir_path(entry, target_dir, static_files_dir);
        if !root.try_exists()? {
            continue
        }

        if fs::metadata(&root)?.is_dir() {
            removed += prune_dir(&root, &listed)?;
        } else {
            removed += remove_if_unlisted(&root, &listed)?;
        }
    }

    info!(target: "reth::cli", removed, "Removed existing files not listed in the snapshot plan");
    Ok(())
}

/// Removes unlisted entries below `dir` without following symlinks, and returns how many files
/// or links it removed.
fn prune_dir(dir: &Path, listed: &HashSet<PathBuf>) -> Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let metadata = std::fs::symlink_metadata(&path)
            .wrap_err_with(|| format!("failed to read metadata of {}", path.display()))?;
        if metadata.is_dir() {
            removed += prune_dir(&path, listed)?;
            remove_dir_if_empty(&path)?;
        } else {
            removed += remove_if_unlisted(&path, listed)?;
        }
    }
    Ok(removed)
}

/// Removes a file or symlink unless it is listed. Removing a symlink leaves its target intact.
fn remove_if_unlisted(path: &Path, listed: &HashSet<PathBuf>) -> Result<usize> {
    if listed.contains(path) {
        return Ok(0)
    }
    let removed = std::fs::remove_file(path);
    // Windows removes a symlink to a directory with `remove_dir`, not `remove_file`.
    #[cfg(windows)]
    let removed = removed.or_else(|_| std::fs::remove_dir(path));
    removed.wrap_err_with(|| format!("failed to remove {}", path.display()))?;
    Ok(1)
}

/// Removes `dir` if pruning left it empty.
fn remove_dir_if_empty(dir: &Path) -> Result<()> {
    if fs::read_dir(dir)?.next().is_none() {
        std::fs::remove_dir(dir)
            .wrap_err_with(|| format!("failed to remove empty directory {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::manifest::{OutputFileChecksum, SnapshotArchive, SnapshotComponentType};
    use tempfile::tempdir;

    fn planned_with_outputs(paths: &[&str]) -> Vec<PlannedArchive> {
        vec![PlannedArchive {
            ty: SnapshotComponentType::State,
            component: "State".to_string(),
            archive: SnapshotArchive {
                url: "https://example.com/state.tar.zst".to_string(),
                file_name: "state.tar.zst".to_string(),
                size: 1,
                blake3: None,
                output_files: paths
                    .iter()
                    .map(|path| OutputFileChecksum {
                        path: (*path).to_string(),
                        size: 1,
                        blake3: String::new(),
                    })
                    .collect(),
            },
        }]
    }

    fn write_file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"data").unwrap();
    }

    #[test]
    fn prune_removes_unlisted_files_and_keeps_listed_ones() {
        // Given: listed snapshot outputs, leftovers in every managed path, and an unmanaged file.
        let dir = tempdir().unwrap();
        let target = dir.path();
        let listed = ["db/mdbx.dat", "static_files/static_file_headers_0_499999"];
        let leftovers = [
            "db/stale.dat",
            "rocksdb/000042.sst",
            "static_files/static_file_headers_500000_999999",
            "reth.toml",
        ];
        for path in listed.iter().chain(&leftovers).chain(&["known-peers.json"]) {
            write_file(&target.join(path));
        }

        // When: pruning against the plan.
        prune_unlisted_outputs(&planned_with_outputs(&listed), target, None).unwrap();

        // Then: only the leftovers under managed paths are gone.
        assert!(listed.iter().all(|path| target.join(path).exists()));
        assert!(leftovers.iter().all(|path| !target.join(path).exists()));
        assert!(target.join("known-peers.json").exists());
    }

    #[test]
    fn prune_removes_nested_directories_it_empties_and_keeps_managed_ones() {
        // Given: a managed path that only contains unlisted files in a nested directory.
        let dir = tempdir().unwrap();
        write_file(&dir.path().join("rocksdb/nested/000042.sst"));

        // When: pruning against a plan that lists nothing under it.
        prune_unlisted_outputs(&planned_with_outputs(&["db/mdbx.dat"]), dir.path(), None).unwrap();

        // Then: the emptied nested directory is removed, and the managed directory is kept.
        assert!(!dir.path().join("rocksdb/nested").exists());
        assert!(dir.path().join("rocksdb").is_dir());
    }

    #[test]
    fn prune_resolves_static_files_in_a_custom_directory() {
        // Given: static files stored outside the data dir.
        let dir = tempdir().unwrap();
        let target = dir.path().join("datadir");
        let static_files = dir.path().join("custom-static-files");
        write_file(&static_files.join("static_file_headers_0_499999"));
        write_file(&static_files.join("static_file_headers_500000_999999"));

        // When: pruning with the custom static files directory.
        prune_unlisted_outputs(
            &planned_with_outputs(&["static_files/static_file_headers_0_499999"]),
            &target,
            Some(&static_files),
        )
        .unwrap();

        // Then: the listed file stays and the unlisted one in the custom directory is removed.
        assert!(static_files.join("static_file_headers_0_499999").exists());
        assert!(!static_files.join("static_file_headers_500000_999999").exists());
    }

    #[test]
    fn prune_skips_missing_managed_paths() {
        // Given: an empty data dir.
        let dir = tempdir().unwrap();

        // When: pruning.
        prune_unlisted_outputs(&planned_with_outputs(&["db/mdbx.dat"]), dir.path(), None).unwrap();

        // Then: nothing is created.
        assert!(!dir.path().join("db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn prune_removes_nested_symlinks_without_following_them() {
        // Given: a symlink inside static_files that points to a directory outside the data dir.
        let dir = tempdir().unwrap();
        let target = dir.path().join("datadir");
        let outside = dir.path().join("outside");
        write_file(&outside.join("unrelated.dat"));
        write_file(&target.join("static_files/static_file_headers_0_499999"));
        std::os::unix::fs::symlink(&outside, target.join("static_files/linked")).unwrap();

        // When: pruning against a plan that does not list the link.
        prune_unlisted_outputs(
            &planned_with_outputs(&["static_files/static_file_headers_0_499999"]),
            &target,
            None,
        )
        .unwrap();

        // Then: the link is removed and the files it pointed to are untouched.
        assert!(std::fs::symlink_metadata(target.join("static_files/linked")).is_err());
        assert!(outside.join("unrelated.dat").exists());
    }

    #[cfg(unix)]
    #[test]
    fn prune_follows_a_symlinked_managed_directory() {
        // Given: static_files is a symlink to a directory on another disk.
        let dir = tempdir().unwrap();
        let target = dir.path().join("datadir");
        let other_disk = dir.path().join("other-disk");
        write_file(&other_disk.join("static_file_headers_0_499999"));
        write_file(&other_disk.join("static_file_headers_500000_999999"));
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&other_disk, target.join("static_files")).unwrap();

        // When: pruning.
        prune_unlisted_outputs(
            &planned_with_outputs(&["static_files/static_file_headers_0_499999"]),
            &target,
            None,
        )
        .unwrap();

        // Then: the link is kept, and only the unlisted file behind it is removed.
        assert!(std::fs::symlink_metadata(target.join("static_files")).unwrap().is_symlink());
        assert!(other_disk.join("static_file_headers_0_499999").exists());
        assert!(!other_disk.join("static_file_headers_500000_999999").exists());
    }
}
