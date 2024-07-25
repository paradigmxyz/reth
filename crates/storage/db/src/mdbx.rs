//! Bindings for [MDBX](https://libmdbx.dqdkfa.ru/).

use std::path::Path;

pub use crate::implementation::mdbx::*;
pub use reth_libmdbx::*;

/// Opens up an existing database or creates a new one at the specified path. Creates tables if
/// necessary. Read/Write mode.
pub fn init_db<P: AsRef<Path>>(path: P, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    todo!()
}

/// Opens up an existing database. Read only mode. It doesn't create it or create tables if missing.
pub fn open_db_read_only(path: &Path, args: DatabaseArguments) -> eyre::Result<DatabaseEnv> {
    todo!()
}
