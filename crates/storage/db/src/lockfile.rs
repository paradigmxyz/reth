//! Storage lock utils.

#![cfg_attr(feature = "disable-lock", allow(dead_code))]

use reth_storage_errors::lockfile::StorageLockError;
use reth_tracing::tracing::error;
use std::{
    path::{Path, PathBuf},
    process,
    sync::{Arc, OnceLock},
};
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

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
        let file_path = path.join(LOCKFILE_NAME);

        #[cfg(feature = "disable-lock")]
        {
            // Too expensive for ef-tests to write/read lock to/from disk.
            Ok(Self(Arc::new(StorageLockInner { file_path })))
        }

        #[cfg(not(feature = "disable-lock"))]
        {
            if let Some(process_lock) = ProcessUID::parse(&file_path)? {
                if process_lock.pid != (process::id() as usize) && process_lock.is_active() {
                    error!(
                        target: "reth::db::lockfile",
                        path = ?file_path,
                        pid = process_lock.pid,
                        start_time = process_lock.start_time,
                        "Storage lock already taken."
                    );
                    return Err(StorageLockError::Taken(process_lock.pid))
                }
            }

            Ok(Self(Arc::new(StorageLockInner::new(file_path)?)))
        }
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

        // Write this process unique identifier (pid & start_time) to file
        ProcessUID::own().write(&file_path)?;

        Ok(Self { file_path })
    }
}

#[derive(Clone, Debug)]
struct ProcessUID {
    /// OS process identifier
    pid: usize,
    /// Process start time
    start_time: u64,
}

impl ProcessUID {
    /// Creates [`Self`] for the provided PID.
    fn new(pid: usize) -> Option<Self> {
        let mut system = System::new();
        let pid2 = sysinfo::Pid::from(pid);
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid2]),
            ProcessRefreshKind::new(),
        );
        system.process(pid2).map(|process| Self { pid, start_time: process.start_time() })
    }

    /// Creates [`Self`] from own process.
    fn own() -> Self {
        static CACHE: OnceLock<ProcessUID> = OnceLock::new();
        CACHE.get_or_init(|| Self::new(process::id() as usize).expect("own process")).clone()
    }

    /// Parses [`Self`] from a file.
    fn parse(path: &Path) -> Result<Option<Self>, StorageLockError> {
        if path.exists() {
            if let Ok(contents) = reth_fs_util::read_to_string(path) {
                let mut lines = contents.lines();
                if let (Some(Ok(pid)), Some(Ok(start_time))) = (
                    lines.next().map(str::trim).map(str::parse),
                    lines.next().map(str::trim).map(str::parse),
                ) {
                    return Ok(Some(Self { pid, start_time }));
                }
            }
        }
        Ok(None)
    }

    /// Whether a process with this `pid` and `start_time` exists.
    fn is_active(&self) -> bool {
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()))
            .process(self.pid.into())
            .is_some_and(|p| p.start_time() == self.start_time)
    }

    /// Writes `pid` and `start_time` to a file.
    fn write(&self, path: &Path) -> Result<(), StorageLockError> {
        Ok(reth_fs_util::write(path, format!("{}\n{}", self.pid, self.start_time))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    // helper to ensure some tests are run serially
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    fn serial_lock() -> MutexGuard<'static, ()> {
        SERIAL.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn test_lock() {
        let _guard = serial_lock();

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
        ProcessUID { pid: fake_pid, start_time: u64::MAX }.write(&lock_file).unwrap();
        assert_eq!(Ok(lock.clone()), StorageLock::try_acquire(temp_dir.path()));

        let mut pid_1 = ProcessUID::new(1).unwrap();

        // If a parsed `ProcessUID` exists, the lock can NOT be acquired.
        pid_1.write(&lock_file).unwrap();
        assert_eq!(Err(StorageLockError::Taken(1)), StorageLock::try_acquire(temp_dir.path()));

        // A lock of a different but existing PID can be acquired ONLY IF the start_time differs.
        pid_1.start_time += 1;
        pid_1.write(&lock_file).unwrap();
        assert_eq!(Ok(lock), StorageLock::try_acquire(temp_dir.path()));
    }

    #[test]
    fn test_drop_lock() {
        let _guard = serial_lock();

        let temp_dir = tempfile::tempdir().unwrap();
        let lock_file = temp_dir.path().join(LOCKFILE_NAME);

        let lock = StorageLock::try_acquire(temp_dir.path()).unwrap();

        assert!(lock_file.exists());
        drop(lock);
        assert!(!lock_file.exists());
    }
}
