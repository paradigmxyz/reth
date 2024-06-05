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
/// This lock stores the PID of the process holding it and is released (deleted) on a graceful
/// shutdown. On resuming from a crash, the stored PID helps verify that no other process holds the
/// lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLock(Arc<StorageLockInner>);

impl StorageLock {
    /// Tries to acquire a write lock on the target directory, returning [`StorageLockError`] if
    /// unsuccessful.
    ///
    /// Note: In-process exclusivity is not on scope. If called from the same process (or another
    /// with the same PID), it will succeed.
    pub fn try_acquire(path: &Path) -> Result<Self, StorageLockError> {
        let path = path.join(LOCKFILE_NAME);

        if let Some(pid) = parse_lock_file_pid(&path)? {
            if pid != (process::id() as usize) && System::new_all().process(pid.into()).is_some() {
                return Err(StorageLockError::Taken(pid))
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

/// Parses the PID from the lock file if it exists.
fn parse_lock_file_pid(path: &Path) -> Result<Option<usize>, StorageLockError> {
    if path.exists() {
        let contents = reth_fs_util::read_to_string(path)?;
        return Ok(contents.trim().parse().ok())
    }
    Ok(None)
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
