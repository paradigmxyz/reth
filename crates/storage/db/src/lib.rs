//! MDBX implementation for reth's database abstraction layer.
//!
//! This crate is an implementation of [`reth-db-api`] for MDBX, as well as a few other common
//! database types.
//!
//! # Overview
//!
//! An overview of the current data model of reth can be found in the [`mod@tables`] module.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod implementation;
pub mod lockfile;
#[cfg(feature = "mdbx")]
mod metrics;
pub mod static_file;
pub mod tables;
#[cfg(feature = "mdbx")]
mod utils;
pub mod version;

#[cfg(feature = "mdbx")]
pub mod mdbx;

pub use reth_storage_errors::db::{DatabaseError, DatabaseWriteOperation};
pub use tables::*;
#[cfg(feature = "mdbx")]
pub use utils::is_database_empty;

#[cfg(feature = "mdbx")]
pub use mdbx::{create_db, init_db, open_db, open_db_read_only, DatabaseEnv, DatabaseEnvKind};

pub use models::ClientVersion;
pub use reth_db_api::*;

/// Collection of database test utilities
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;
    use crate::mdbx::DatabaseArguments;
    use parking_lot::RwLock;
    use reth_db_api::{
        database::Database,
        database_metrics::{DatabaseMetadata, DatabaseMetadataValue, DatabaseMetrics},
        models::ClientVersion,
    };
    use reth_fs_util;
    use reth_libmdbx::MaxReadTransactionDuration;
    use std::{
        fmt::Formatter,
        path::{Path, PathBuf},
        sync::Arc,
    };
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
    pub struct TempDatabase<DB> {
        db: Option<DB>,
        path: PathBuf,
        /// Executed right before a database transaction is created.
        pre_tx_hook: RwLock<Box<dyn Fn() + Send + Sync>>,
        /// Executed right after a database transaction is created.
        post_tx_hook: RwLock<Box<dyn Fn() + Send + Sync>>,
    }

    impl<DB: std::fmt::Debug> std::fmt::Debug for TempDatabase<DB> {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TempDatabase").field("db", &self.db).field("path", &self.path).finish()
        }
    }

    impl<DB> Drop for TempDatabase<DB> {
        fn drop(&mut self) {
            if let Some(db) = self.db.take() {
                drop(db);
                let _ = reth_fs_util::remove_dir_all(&self.path);
            }
        }
    }

    impl<DB> TempDatabase<DB> {
        /// Create new [`TempDatabase`] instance.
        pub fn new(db: DB, path: PathBuf) -> Self {
            Self {
                db: Some(db),
                path,
                pre_tx_hook: RwLock::new(Box::new(|| ())),
                post_tx_hook: RwLock::new(Box::new(|| ())),
            }
        }

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

        /// Sets [`TempDatabase`] new pre transaction creation hook.
        pub fn set_pre_transaction_hook(&self, hook: Box<dyn Fn() + Send + Sync>) {
            let mut db_hook = self.pre_tx_hook.write();
            *db_hook = hook;
        }

        /// Sets [`TempDatabase`] new post transaction creation hook.
        pub fn set_post_transaction_hook(&self, hook: Box<dyn Fn() + Send + Sync>) {
            let mut db_hook = self.post_tx_hook.write();
            *db_hook = hook;
        }
    }

    impl<DB: Database> Database for TempDatabase<DB> {
        type TX = <DB as Database>::TX;
        type TXMut = <DB as Database>::TXMut;
        fn tx(&self) -> Result<Self::TX, DatabaseError> {
            self.pre_tx_hook.read()();
            let tx = self.db().tx()?;
            self.post_tx_hook.read()();
            Ok(tx)
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

    /// Create `static_files` path for testing
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

        Arc::new(TempDatabase::new(db, path))
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
        Arc::new(TempDatabase::new(db, path))
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
        Arc::new(TempDatabase::new(db, path))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        init_db,
        mdbx::DatabaseArguments,
        open_db, tables,
        version::{db_version_file_path, DatabaseVersionError},
    };
    use assert_matches::assert_matches;
    use reth_db_api::{
        cursor::DbCursorRO, database::Database, models::ClientVersion, transaction::DbTx,
    };
    use reth_libmdbx::MaxReadTransactionDuration;
    use std::time::Duration;
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
            reth_fs_util::write(path.path().join(db_version_file_path(&path)), "invalid-version")
                .unwrap();
            let db = init_db(&path, args.clone());
            assert!(db.is_err());
            assert_matches!(
                db.unwrap_err().downcast_ref::<DatabaseVersionError>(),
                Some(DatabaseVersionError::MalformedFile)
            )
        }

        // Database is not empty, version file contains not matching version
        {
            reth_fs_util::write(path.path().join(db_version_file_path(&path)), "0").unwrap();
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
