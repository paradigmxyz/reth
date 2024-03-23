//! reth's database abstraction layer with concrete implementations.
//!
//! The database abstraction assumes that the underlying store is a KV store subdivided into tables.
//!
//! One or more changes are tied to a transaction that is atomically committed to the data store at
//! the same time. Strong consistency in what data is written and when is important for reth, so it
//! is not possible to write data to the database outside of using a transaction.
//!
//! Good starting points for this crate are:
//!
//! - [`Database`] for the main database abstraction
//! - [`DbTx`] (RO) and [`DbTxMut`] (RW) for the transaction abstractions.
//! - [`DbCursorRO`] (RO) and [`DbCursorRW`] (RW) for the cursor abstractions (see below).
//!
//! # Cursors and Walkers
//!
//! The abstraction also defines a couple of helpful abstractions for iterating and writing data:
//!
//! - **Cursors** ([`DbCursorRO`] / [`DbCursorRW`]) for iterating data in a table. Cursors are
//!   assumed to resolve data in a sorted manner when iterating from start to finish, and it is safe
//!   to assume that they are efficient at doing so.
//! - **Walkers** ([`Walker`] / [`RangeWalker`] / [`ReverseWalker`]) use cursors to walk the entries
//!   in a table, either fully from a specific point, or over a range.
//!
//! Dup tables (see below) also have corresponding cursors and walkers (e.g. [`DbDupCursorRO`]).
//! These **should** be preferred when working with dup tables, as they provide additional methods
//! that are optimized for dup tables.
//!
//! # Tables
//!
//! reth has two types of tables: simple KV stores (one key, one value) and dup tables (one key,
//! many values). Dup tables can be efficient for certain types of data.
//!
//! Keys are de/serialized using the [`Encode`] and [`Decode`] traits, and values are de/serialized
//! ("compressed") using the [`Compress`] and [`Decompress`] traits.
//!
//! Tables implement the [`Table`] trait.
//!
//! # Overview
//!
//! An overview of the current data model of reth can be found in the [`tables`] module.
//!
//! [`Database`]: crate::abstraction::database::Database
//! [`DbTx`]: crate::abstraction::transaction::DbTx
//! [`DbTxMut`]: crate::abstraction::transaction::DbTxMut
//! [`DbCursorRO`]: crate::abstraction::cursor::DbCursorRO
//! [`DbCursorRW`]: crate::abstraction::cursor::DbCursorRW
//! [`Walker`]: crate::abstraction::cursor::Walker
//! [`RangeWalker`]: crate::abstraction::cursor::RangeWalker
//! [`ReverseWalker`]: crate::abstraction::cursor::ReverseWalker
//! [`DbDupCursorRO`]: crate::abstraction::cursor::DbDupCursorRO
//! [`Encode`]: crate::abstraction::table::Encode
//! [`Decode`]: crate::abstraction::table::Decode
//! [`Compress`]: crate::abstraction::table::Compress
//! [`Decompress`]: crate::abstraction::table::Decompress
//! [`Table`]: crate::abstraction::table::Table

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Traits defining the database abstractions, such as cursors and transactions.
pub mod abstraction;

mod implementation;
mod metrics;
pub mod static_file;
pub mod tables;
mod utils;
pub mod version;

#[cfg(feature = "mdbx")]
/// Bindings for [MDBX](https://libmdbx.dqdkfa.ru/).
pub mod mdbx {
    pub use crate::implementation::mdbx::*;
    pub use reth_libmdbx::*;
}

pub use abstraction::*;
pub use reth_interfaces::db::{DatabaseError, DatabaseWriteOperation};
pub use tables::*;
pub use utils::is_database_empty;

#[cfg(feature = "mdbx")]
pub use mdbx::{DatabaseEnv, DatabaseEnvKind};

use crate::mdbx::DatabaseArguments;
use eyre::WrapErr;
use std::path::Path;

/// Creates a new database at the specified path if it doesn't exist. Does NOT create tables. Check
/// [`init_db`].
pub fn create_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    use crate::version::{check_db_version_file, create_db_version_file, DatabaseVersionError};

    let rpath = path.as_ref();
    if is_database_empty(rpath) {
        reth_primitives::fs::create_dir_all(rpath)
            .wrap_err_with(|| format!("Could not create database directory {}", rpath.display()))?;
        create_db_version_file(rpath)?;
    } else {
        match check_db_version_file(rpath) {
            Ok(_) => (),
            Err(DatabaseVersionError::MissingFile) => create_db_version_file(rpath)?,
            Err(err) => return Err(err.into()),
        }
    }

    #[cfg(feature = "mdbx")]
    {
        Ok(DatabaseEnv::open(rpath, DatabaseEnvKind::RW, args)?)
    }
    #[cfg(not(feature = "mdbx"))]
    {
        unimplemented!();
    }
}

/// Opens up an existing database or creates a new one at the specified path. Creates tables if
/// necessary. Read/Write mode.
pub fn init_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    #[cfg(feature = "mdbx")]
    {
        let client_version = args.client_version().clone();
        let db = create_db(path, args)?;
        db.create_tables()?;
        db.record_client_version(client_version)?;
        Ok(db)
    }
    #[cfg(not(feature = "mdbx"))]
    {
        unimplemented!();
    }
}

/// Opens up an existing database. Read only mode. It doesn't create it or create tables if missing.
pub fn open_db_read_only(path: &Path, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    #[cfg(feature = "mdbx")]
    {
        DatabaseEnv::open(path, DatabaseEnvKind::RO, args)
            .with_context(|| format!("Could not open database at path: {}", path.display()))
    }
    #[cfg(not(feature = "mdbx"))]
    {
        unimplemented!();
    }
}

/// Opens up an existing database. Read/Write mode with WriteMap enabled. It doesn't create it or
/// create tables if missing.
pub fn open_db(path: &Path, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    #[cfg(feature = "mdbx")]
    {
        let db = DatabaseEnv::open(path, DatabaseEnvKind::RW, args.clone())
            .with_context(|| format!("Could not open database at path: {}", path.display()))?;
        db.record_client_version(args.client_version().clone())?;
        Ok(db)
    }
    #[cfg(not(feature = "mdbx"))]
    {
        unimplemented!();
    }
}

/// Collection of database test utilities
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;
    use crate::{
        database::Database,
        database_metrics::{DatabaseMetadata, DatabaseMetadataValue, DatabaseMetrics},
        models::client_version::ClientVersion,
    };
    use reth_libmdbx::MaxReadTransactionDuration;
    use reth_primitives::fs;
    use std::{path::PathBuf, sync::Arc};
    use tempfile::TempDir;

    /// Error during database open
    pub const ERROR_DB_OPEN: &str = "Not able to open the database file.";
    /// Error during database creation
    pub const ERROR_DB_CREATION: &str = "Not able to create the database file.";
    /// Error during database creation
    pub const ERROR_STATIC_FILES_CREATION: &str = "Not able to create the static file path.";
    /// Error during table creation
    pub const ERROR_TABLE_CREATION: &str = "Not able to create tables in the database.";
    /// Error during tempdir creation
    pub const ERROR_TEMPDIR: &str = "Not able to create a temporary directory.";

    /// A database will delete the db dir when dropped.
    #[derive(Debug)]
    pub struct TempDatabase<DB> {
        db: Option<DB>,
        path: PathBuf,
    }

    impl<DB> Drop for TempDatabase<DB> {
        fn drop(&mut self) {
            if let Some(db) = self.db.take() {
                drop(db);
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    impl<DB> TempDatabase<DB> {
        /// Returns the reference to inner db.
        pub fn db(&self) -> &DB {
            self.db.as_ref().unwrap()
        }

        /// Returns the path to the database.
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Convert temp database into inner.
        pub fn into_inner_db(mut self) -> DB {
            self.db.take().unwrap() // take out db to avoid clean path in drop fn
        }
    }

    impl<DB: Database> Database for TempDatabase<DB> {
        type TX = <DB as Database>::TX;
        type TXMut = <DB as Database>::TXMut;
        fn tx(&self) -> Result<Self::TX, DatabaseError> {
            self.db().tx()
        }

        fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
            self.db().tx_mut()
        }
    }

    impl<DB: DatabaseMetrics> DatabaseMetrics for TempDatabase<DB> {
        fn report_metrics(&self) {
            self.db().report_metrics()
        }
    }

    impl<DB: DatabaseMetadata> DatabaseMetadata for TempDatabase<DB> {
        fn metadata(&self) -> DatabaseMetadataValue {
            self.db().metadata()
        }
    }

    /// Create static_files path for testing
    pub fn create_test_static_files_dir() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::with_prefix("reth-test-static-").expect(ERROR_TEMPDIR);
        let path = temp_dir.path().to_path_buf();
        (temp_dir, path)
    }

    /// Get a temporary directory path to use for the database
    pub fn tempdir_path() -> PathBuf {
        let builder = tempfile::Builder::new().prefix("reth-test-").rand_bytes(8).tempdir();
        builder.expect(ERROR_TEMPDIR).into_path()
    }

    /// Create read/write database for testing
    pub fn create_test_rw_db() -> Arc<TempDatabase<DatabaseEnv>> {
        let path = tempdir_path();
        let emsg = format!("{ERROR_DB_CREATION}: {path:?}");

        let db = init_db(
            &path,
            DatabaseArguments::new(ClientVersion::default())
                .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded)),
        )
        .expect(&emsg);

        Arc::new(TempDatabase { db: Some(db), path })
    }

    /// Create read/write database for testing
    pub fn create_test_rw_db_with_path<P: AsRef<Path>>(path: P) -> Arc<TempDatabase<DatabaseEnv>> {
        let path = path.as_ref().to_path_buf();
        let db = init_db(
            path.as_path(),
            DatabaseArguments::new(ClientVersion::default())
                .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded)),
        )
        .expect(ERROR_DB_CREATION);
        Arc::new(TempDatabase { db: Some(db), path })
    }

    /// Create read only database for testing
    pub fn create_test_ro_db() -> Arc<TempDatabase<DatabaseEnv>> {
        let args = DatabaseArguments::new(ClientVersion::default())
            .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded));

        let path = tempdir_path();
        {
            init_db(path.as_path(), args.clone()).expect(ERROR_DB_CREATION);
        }
        let db = open_db_read_only(path.as_path(), args).expect(ERROR_DB_OPEN);
        Arc::new(TempDatabase { db: Some(db), path })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        cursor::DbCursorRO,
        database::Database,
        init_db,
        mdbx::DatabaseArguments,
        models::client_version::ClientVersion,
        open_db, tables,
        transaction::DbTx,
        version::{db_version_file_path, DatabaseVersionError},
    };
    use assert_matches::assert_matches;
    use reth_libmdbx::MaxReadTransactionDuration;
    use reth_primitives::fs;
    use tempfile::tempdir;

    #[test]
    fn db_version() {
        let path = tempdir().unwrap();

        let args = DatabaseArguments::new(ClientVersion::default())
            .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Unbounded));

        // Database is empty
        {
            let db = init_db(&path, args.clone());
            assert_matches!(db, Ok(_));
        }

        // Database is not empty, current version is the same as in the file
        {
            let db = init_db(&path, args.clone());
            assert_matches!(db, Ok(_));
        }

        // Database is not empty, version file is malformed
        {
            fs::write(path.path().join(db_version_file_path(&path)), "invalid-version").unwrap();
            let db = init_db(&path, args.clone());
            assert!(db.is_err());
            assert_matches!(
                db.unwrap_err().downcast_ref::<DatabaseVersionError>(),
                Some(DatabaseVersionError::MalformedFile)
            )
        }

        // Database is not empty, version file contains not matching version
        {
            fs::write(path.path().join(db_version_file_path(&path)), "0").unwrap();
            let db = init_db(&path, args);
            assert!(db.is_err());
            assert_matches!(
                db.unwrap_err().downcast_ref::<DatabaseVersionError>(),
                Some(DatabaseVersionError::VersionMismatch { version: 0 })
            )
        }
    }

    #[test]
    fn db_client_version() {
        let path = tempdir().unwrap();

        // Empty client version is not recorded
        {
            let db = init_db(&path, DatabaseArguments::new(ClientVersion::default())).unwrap();
            let tx = db.tx().unwrap();
            let mut cursor = tx.cursor_read::<tables::VersionHistory>().unwrap();
            assert_matches!(cursor.first(), Ok(None));
        }

        // Client version is recorded
        let first_version = ClientVersion { version: String::from("v1"), ..Default::default() };
        {
            let db = init_db(&path, DatabaseArguments::new(first_version.clone())).unwrap();
            let tx = db.tx().unwrap();
            let mut cursor = tx.cursor_read::<tables::VersionHistory>().unwrap();
            assert_eq!(
                cursor
                    .walk_range(..)
                    .unwrap()
                    .map(|x| x.map(|(_, v)| v))
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                vec![first_version.clone()]
            );
        }

        // Same client version is not duplicated.
        {
            let db = init_db(&path, DatabaseArguments::new(first_version.clone())).unwrap();
            let tx = db.tx().unwrap();
            let mut cursor = tx.cursor_read::<tables::VersionHistory>().unwrap();
            assert_eq!(
                cursor
                    .walk_range(..)
                    .unwrap()
                    .map(|x| x.map(|(_, v)| v))
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                vec![first_version.clone()]
            );
        }

        // Different client version is recorded
        std::thread::sleep(Duration::from_secs(1));
        let second_version = ClientVersion { version: String::from("v2"), ..Default::default() };
        {
            let db = init_db(&path, DatabaseArguments::new(second_version.clone())).unwrap();
            let tx = db.tx().unwrap();
            let mut cursor = tx.cursor_read::<tables::VersionHistory>().unwrap();
            assert_eq!(
                cursor
                    .walk_range(..)
                    .unwrap()
                    .map(|x| x.map(|(_, v)| v))
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                vec![first_version.clone(), second_version.clone()]
            );
        }

        // Different client version is recorded on db open.
        std::thread::sleep(Duration::from_secs(1));
        let third_version = ClientVersion { version: String::from("v3"), ..Default::default() };
        {
            let db = open_db(path.path(), DatabaseArguments::new(third_version.clone())).unwrap();
            let tx = db.tx().unwrap();
            let mut cursor = tx.cursor_read::<tables::VersionHistory>().unwrap();
            assert_eq!(
                cursor
                    .walk_range(..)
                    .unwrap()
                    .map(|x| x.map(|(_, v)| v))
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                vec![first_version, second_version, third_version]
            );
        }
    }
}
