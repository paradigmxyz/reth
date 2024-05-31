//! Storage lock utils.

use reth_storage_errors::lockfile::StorageLockError;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process,
    sync::Arc,
};
use sysinfo::System;

/// A lock for a storage directory to ensure exclusive read-write access.
///
/// This lock stores the PID of the process holding it and is released on a graceful shutdown.
/// On resuming from a crash, the stored PID helps verify that no other process holds the lock.
#[derive(Debug, Clone)]
pub struct StorageLock(Arc<StorageLockInner>);

impl StorageLock {
    /// Tries to acquires a write lock on the target directory, returning [StorageLockError] if
    /// unsuccessful.
    pub fn try_acquire(path: &Path) -> Result<Self, StorageLockError> {
        let path = path.join("lock");
        let lock = match parse_lock_file_pid(&path)? {
            Some(pid) => {
                if System::new_all().process(pid.into()).is_some() {
                    return Err(StorageLockError::Taken)
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
        if Arc::strong_count(&self.0) == 1 {
            if let Err(e) = std::fs::remove_file(&self.0.path) {
                eprintln!("Failed to delete lock file: {}", e);
            }
        }
    }
}

#[derive(Debug)]
struct StorageLockInner {
    _file: File,
    path: PathBuf,
}

impl StorageLockInner {
    /// Creates lock file and writes this process PID into it.
    fn new(file_path: impl AsRef<Path>) -> Result<Self, StorageLockError> {
        let path = file_path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().create(true).write(true).open(&path)?;
        write!(file, "{}", process::id() as usize)?;
        Ok(Self { _file: file, path })
    }
}

/// Parses the PID from the lock file if it exists.
fn parse_lock_file_pid(path: impl AsRef<Path>) -> Result<Option<usize>, StorageLockError> {
    if path.as_ref().exists() {
        let mut file = File::open(path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        return Ok(contents.trim().parse().ok())
    }
    Ok(None)
}
