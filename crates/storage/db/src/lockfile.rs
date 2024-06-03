//! Storage lock utils.

use reth_storage_errors::lockfile::StorageLockError;
use reth_tracing::tracing::error;
use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};
use sysinfo::System;

/// A file lock for a storage directory to ensure exclusive read-write access.
///
/// This lock stores the PID of the process holding it and is released (deleted) on a graceful
/// shutdown. On resuming from a crash, the stored PID helps verify that no other process holds the
/// lock.
#[derive(Debug, Clone)]
pub struct StorageLock(Arc<StorageLockInner>);

impl StorageLock {
    /// Tries to acquire a write lock on the target directory, returning [`StorageLockError`] if
    /// unsuccessful.
    pub fn try_acquire(path: &Path) -> Result<Self, StorageLockError> {
        let path = path.join("lock");
        let lock = match parse_lock_file_pid(&path)? {
            Some(pid) => {
                if System::new_all().process(pid.into()).is_some() {
                    return Err(StorageLockError::Taken(pid))
                } else {
                    // If PID is no longer active, take hold of the lock.
                    StorageLockInner::new(path)
                }
            }
            None => StorageLockInner::new(path),
        };
        Ok(Self(Arc::new(lock?)))
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

#[derive(Debug)]
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
