//! Storage lock utils.

use reth_storage_errors::lockfile::StorageLockError;
use reth_tracing::tracing::error;
use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};
use sysinfo::System;

/// File lock name.
const LOCKFILE_NAME: &str = "lock";

/// A file lock for a storage directory to ensure exclusive read-write access across different
/// processes.
///
/// This lock stores the PID and start time of the process holding it and is released (deleted)
/// on a graceful shutdown. On resuming from a crash, the stored data helps to verify that no other
/// process holds the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLock(Arc<StorageLockInner>);

impl StorageLock {
    /// Tries to acquire a write lock on the target directory, returning [`StorageLockError`] if
    /// unsuccessful.
    ///
    /// Note: In-process exclusivity is not on scope. If called from the same process (or another
    /// with the same PID and start time), it will succeed.
    pub fn try_acquire(path: &Path) -> Result<Self, StorageLockError> {
        let path = path.join(LOCKFILE_NAME);

        if let (Some(pid), start_time) = parse_lock_file(&path)? {
            // Check if the process with the stored PID is different from the current process.
            if pid != (process::id() as usize) {
                let system = System::new_all();
                let process = system.process(pid.into());
                // Check if the process with the stored PID is still running.
                if let Some(process) = process {
                    // If we couldn't parse the start time from the file, or the process has the
                    // same start time as the stored start time, we assume the lock is still taken.
                    if start_time.map_or(true, |start_time| start_time == process.start_time()) {
                        return Err(StorageLockError::Taken(pid))
                    }
                }
            }
        }

        Ok(Self(Arc::new(StorageLockInner::new(path)?)))
    }
}

impl Drop for StorageLock {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) == 1 && self.0.file_path.exists() {
            // TODO: should only happen during tests that the file does not exist: tempdir is
            // getting dropped first. However, tempdir shouldn't be dropped
            // before any of the storage providers.
            if let Err(err) = reth_fs_util::remove_file(&self.0.file_path) {
                error!(%err, "Failed to delete lock file");
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StorageLockInner {
    file_path: PathBuf,
}

impl StorageLockInner {
    /// Creates lock file and writes this process PID into it.
    fn new(file_path: PathBuf) -> Result<Self, StorageLockError> {
        // Create the directory if it doesn't exist
        if let Some(parent) = file_path.parent() {
            reth_fs_util::create_dir_all(parent)?;
        }

        reth_fs_util::write(&file_path, format!("{}", process::id()))?;

        Ok(Self { file_path })
    }
}

/// Parses the PID and the start time from the lock file, if it exists.
///
/// Returns:
/// - `None` for both values if the file does not exist or we couldn't parse the contents.
/// - `Some` for PID and `None` for start time if the file was created on an older version.
/// - `Some` for both values if the file was created on a newer version.
fn parse_lock_file(path: &Path) -> Result<(Option<usize>, Option<u64>), StorageLockError> {
    if path.exists() {
        let contents = reth_fs_util::read_to_string(path)?;
        let (pid, start_time) = if let Some((pid, start_time)) = contents.split_once('_') {
            let pid = pid.trim().parse().ok();
            let start_time = start_time.trim().parse().ok();

            if pid.is_some() && start_time.is_some() {
                (pid, start_time)
            } else {
                (None, None)
            }
        } else {
            (contents.trim().parse().ok(), None)
        };

        Ok((pid, start_time))
    } else {
        Ok((None, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock() {
        let temp_dir = tempfile::tempdir().unwrap();

        let lock = StorageLock::try_acquire(temp_dir.path()).unwrap();

        // Same process can re-acquire the lock
        assert_eq!(Ok(lock.clone()), StorageLock::try_acquire(temp_dir.path()));

        // A lock of a non existent PID can be acquired.
        let lock_file = temp_dir.path().join(LOCKFILE_NAME);
        let mut fake_pid = 1337;
        let system = System::new_all();
        while system.process(fake_pid.into()).is_some() {
            fake_pid += 1;
        }
        reth_fs_util::write(&lock_file, format!("{}", fake_pid)).unwrap();
        assert_eq!(Ok(lock), StorageLock::try_acquire(temp_dir.path()));

        // A lock of a different but existing PID cannot be acquired.
        reth_fs_util::write(&lock_file, "1").unwrap();
        assert_eq!(Err(StorageLockError::Taken(1)), StorageLock::try_acquire(temp_dir.path()));
    }

    #[test]
    fn test_drop_lock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let lock_file = temp_dir.path().join(LOCKFILE_NAME);

        let lock = StorageLock::try_acquire(temp_dir.path()).unwrap();

        drop(lock);

        assert!(!lock_file.exists());
    }
}
