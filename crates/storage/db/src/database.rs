//! Helper functions for initializing and opening a database.

use crate::{TableSet, Tables};
use eyre::Context;
use std::path::Path;

// Type aliases for backend-specific types - resolved at compile time
#[cfg(all(feature = "mdbx", not(feature = "rocksdb")))]
use crate::implementation::mdbx::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};

#[cfg(all(feature = "rocksdb", not(feature = "mdbx")))]
use crate::implementation::rocksdb::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};

#[cfg(all(feature = "mdbx", feature = "rocksdb"))]
use crate::implementation::rocksdb::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind};

/// Shared constants for database operations
/// 1 KB in bytes
pub const KILOBYTE: usize = 1024;
/// 1 MB in bytes
pub const MEGABYTE: usize = KILOBYTE * 1024;
/// 1 GB in bytes
pub const GIGABYTE: usize = MEGABYTE * 1024;
/// 1 TB in bytes
pub const TERABYTE: usize = GIGABYTE * 1024;

/// Shared table statistics structure for compatibility between MDBX and RocksDB
#[derive(Debug)]
pub struct TableStats {
    /// Number of entries in the table
    pub entries: usize,
    /// Page size in bytes
    pub page_size: u32,
    /// Number of leaf pages
    pub leaf_pages: usize,
    /// Number of branch pages
    pub branch_pages: usize,
    /// Number of overflow pages
    pub overflow_pages: usize,
}

impl TableStats {
    /// Get the number of entries in the table
    pub fn entries(&self) -> usize {
        self.entries
    }

    /// Get the page size in bytes
    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    /// Get the number of leaf pages
    pub fn leaf_pages(&self) -> usize {
        self.leaf_pages
    }

    /// Get the number of branch pages
    pub fn branch_pages(&self) -> usize {
        self.branch_pages
    }

    /// Get the number of overflow pages
    pub fn overflow_pages(&self) -> usize {
        self.overflow_pages
    }
}

/// Re-export cursor types for unified access
pub mod cursor {
    #[cfg(all(feature = "mdbx", not(feature = "rocksdb")))]
    pub use crate::implementation::mdbx::{Cursor, RO, RW};

    #[cfg(all(feature = "rocksdb", not(feature = "mdbx")))]
    pub use crate::implementation::rocksdb::{Cursor, RO, RW};

    #[cfg(all(feature = "mdbx", feature = "rocksdb"))]
    pub use crate::implementation::rocksdb::{Cursor, RO, RW};
}

fn is_database_empty<P: AsRef<Path>>(path: P) -> bool {
    let path = path.as_ref();

    if !path.exists() {
        true
    } else if path.is_file() {
        false
    } else if let Ok(dir) = path.read_dir() {
        dir.count() == 0
    } else {
        true
    }
}

/// Creates a new database at the specified path if it doesn't exist. Does NOT create tables. Check
/// [`init_db`].
pub fn create_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    use crate::version::{check_db_version_file, create_db_version_file, DatabaseVersionError};

    let rpath = path.as_ref();
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
fn init_db_for<P: AsRef<Path>, TS: TableSet>(
    path: P,
    args: DatabaseArguments,
) -> eyre::Result<DatabaseEnv> {
    let client_version = args.client_version().clone();
    let mut db = create_db(path, args)?;
    db.create_tables_for::<TS>()?;
    db.record_client_version(client_version)?;
    Ok(db)
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
