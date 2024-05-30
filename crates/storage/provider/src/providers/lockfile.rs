use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process,
    sync::Arc,
};
use sysinfo::System;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct StorageLock(Arc<StorageLockInner>);

impl StorageLock {
    /// Acquires a lock for each type of storage. It will return [Option::None] if any of the locks
    /// fails to be acquired.
    pub(crate) fn acquire(database_path: &Path, static_file_path: &Path) -> Option<Self> {
        match (
            acquire_storage_kind_lock(database_path.join("lockfile")),
            acquire_storage_kind_lock(static_file_path.join("lockfile")),
        ) {
            (Some(database_lock), Some(static_file_lock)) => {
                Some(Self(Arc::new(StorageLockInner { static_file_lock, database_lock })))
            }
            _ => None,
        }
    }
}

#[derive(Debug)]
struct StorageLockInner {
    /// Static files lockfile.
    #[allow(dead_code)]
    static_file_lock: File,
    /// Database lockfile.
    #[allow(dead_code)]
    database_lock: File,
}

fn acquire_storage_kind_lock(path: impl AsRef<Path>) -> Option<File> {
    match check_lock_file(&path) {
        Some(pid) => {
            if is_process_running(pid) {
                None
            } else {
                create_lock_file(path)
            }
        }
        None => create_lock_file(path),
    }
}

fn check_lock_file(path: impl AsRef<Path>) -> Option<usize> {
    if path.as_ref().exists() {
        let mut file = File::open(path).expect("Unable to open lock file");
        let mut contents = String::new();
        file.read_to_string(&mut contents).expect("Unable to read lock file");
        contents.trim().parse().ok()
    } else {
        None
    }
}

fn create_lock_file(path: impl AsRef<Path>) -> Option<File> {
    let mut file =
        OpenOptions::new().create(true).write(true).open(path).expect("Unable to create lock file");
    write!(file, "{}", process::id() as usize).expect("Unable to write to lock file");
    Some(file)
}

fn is_process_running(pid: usize) -> bool {
    let system = System::new_all();
    system.process(pid.into()).is_some()
}
