//! Bindings for [MDBX](https://libmdbx.dqdkfa.ru/).

use crate::is_database_empty;
use eyre::Context;
use std::path::Path;

pub use crate::implementation::mdbx::*;
pub use reth_libmdbx::*;

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

/// Opens up an existing database or creates a new one at the specified path. Creates tables if
/// necessary. Read/Write mode.
pub fn init_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    let client_version = args.client_version().clone();
    let db = create_db(path, args)?;
    db.create_tables()?;
    db.record_client_version(client_version)?;
    Ok(db)
}

/// Opens up an existing database. Read only mode. It doesn't create it or create tables if missing.
pub fn open_db_read_only(path: &Path, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    DatabaseEnv::open(path, DatabaseEnvKind::RO, args)
        .with_context(|| format!("Could not open database at path: {}", path.display()))
}

/// Opens up an existing database. Read/Write mode with `WriteMap` enabled. It doesn't create it or
/// create tables if missing.
pub fn open_db(path: &Path, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    let db = DatabaseEnv::open(path, DatabaseEnvKind::RW, args.clone())
        .with_context(|| format!("Could not open database at path: {}", path.display()))?;
    db.record_client_version(args.client_version().clone())?;
    Ok(db)
}
