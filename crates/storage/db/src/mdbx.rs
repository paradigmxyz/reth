//! Helper functions for initializing and opening a database.

use crate::{is_database_empty, TableSet, Tables};
use eyre::Context;
use reth_tracing::tracing::{info, warn};
use std::path::Path;

pub use crate::implementation::mdbx::*;
pub use reth_libmdbx::*;

/// Tables that have been removed from the schema but may still exist on disk from previous
/// versions. These will be dropped during database initialization.
const ORPHAN_TABLES: &[&str] = &["AccountsTrieChangeSets", "StoragesTrieChangeSets"];

/// Checks if the given path resides on a ZFS filesystem and logs a warning.
///
/// ZFS uses copy-on-write (COW) semantics which conflict with MDBX's write patterns, leading to
/// significant performance degradation.
fn warn_if_zfs(path: &Path) {
    if matches!(is_zfs(path), Ok(true)) {
        warn!(
            target: "reth::db",
            path = %path.display(),
            "Database is on a ZFS filesystem. ZFS's copy-on-write behavior causes significant \
             performance degradation with MDBX. Consider using ext4 or xfs instead."
        );
    }
}

/// Returns `true` if the given path is on a ZFS filesystem.
#[cfg(target_os = "linux")]
fn is_zfs(path: &Path) -> std::io::Result<bool> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    /// ZFS filesystem magic number.
    const ZFS_SUPER_MAGIC: i64 = 0x2fc12fc1;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    unsafe {
        let mut stat: libc::statfs = std::mem::zeroed();
        if libc::statfs(c_path.as_ptr(), &raw mut stat) == 0 {
            Ok(stat.f_type == ZFS_SUPER_MAGIC)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// Returns `true` if the given path is on a ZFS filesystem.
#[cfg(target_os = "macos")]
fn is_zfs(path: &Path) -> std::io::Result<bool> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    unsafe {
        let mut stat: libc::statfs = std::mem::zeroed();
        if libc::statfs(c_path.as_ptr(), &raw mut stat) == 0 {
            let fstype = std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr());
            Ok(fstype.to_bytes() == b"zfs")
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// ZFS detection is unsupported on this platform, always returns `Ok(false)`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn is_zfs(_path: &Path) -> std::io::Result<bool> {
    Ok(false)
}

/// Creates a new database at the specified path if it doesn't exist. Does NOT create tables. Check
/// [`init_db`].
pub fn create_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    use crate::version::{check_db_version_file, create_db_version_file, DatabaseVersionError};

    let rpath = path.as_ref();
    warn_if_zfs(rpath);

    if is_database_empty(rpath) {
        reth_fs_util::create_dir_all(rpath)
            .wrap_err_with(|| format!("Could not create database directory {}", rpath.display()))?;
        create_db_version_file(rpath)?;
    } else {
        match check_db_version_file(rpath) {
            Ok(_) => (),
            Err(DatabaseVersionError::MissingFile) => create_db_version_file(rpath)?,
            Err(err) => return Err(err.into()),
        }
    }

    Ok(DatabaseEnv::open(rpath, DatabaseEnvKind::RW, args)?)
}

/// Opens up an existing database or creates a new one at the specified path. Creates tables defined
/// in [`Tables`] if necessary. Read/Write mode.
pub fn init_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    init_db_for::<P, Tables>(path, args)
}

/// Opens up an existing database or creates a new one at the specified path. Creates tables defined
/// in the given [`TableSet`] if necessary. Read/Write mode.
pub fn init_db_for<P: AsRef<Path>, TS: TableSet>(
    path: P,
    args: DatabaseArguments,
) -> eyre::Result<DatabaseEnv> {
    let client_version = args.client_version().clone();
    let mut db = create_db(path, args)?;
    db.create_and_track_tables_for::<TS>()?;
    db.record_client_version(client_version)?;
    drop_orphan_tables(&db);
    Ok(db)
}

/// Drops orphaned tables that are no longer part of the schema.
fn drop_orphan_tables(db: &DatabaseEnv) {
    for table_name in ORPHAN_TABLES {
        match db.drop_orphan_table(table_name) {
            Ok(true) => {
                info!(target: "reth::db", table = %table_name, "Dropped orphaned database table");
            }
            Ok(false) => {}
            Err(e) => {
                reth_tracing::tracing::warn!(
                    target: "reth::db",
                    table = %table_name,
                    %e,
                    "Failed to drop orphaned database table"
                );
            }
        }
    }
}

/// Opens up an existing database. Read only mode. It doesn't create it or create tables if missing.
pub fn open_db_read_only(
    path: impl AsRef<Path>,
    args: DatabaseArguments,
) -> eyre::Result<DatabaseEnv> {
    let path = path.as_ref();
    DatabaseEnv::open(path, DatabaseEnvKind::RO, args)
        .with_context(|| format!("Could not open database at path: {}", path.display()))
}

/// Opens up an existing database. Read/Write mode with `WriteMap` enabled. It doesn't create it or
/// create tables if missing.
pub fn open_db(path: impl AsRef<Path>, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    fn open(path: &Path, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
        let client_version = args.client_version().clone();
        let db = DatabaseEnv::open(path, DatabaseEnvKind::RW, args)
            .with_context(|| format!("Could not open database at path: {}", path.display()))?;
        db.record_client_version(client_version)?;
        Ok(db)
    }
    open(path.as_ref(), args)
}
