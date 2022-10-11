//! Module that interacts with MDBX.

use crate::utils::{default_page_size, TableType};
use libmdbx::{
    DatabaseFlags, Environment, EnvironmentFlags, EnvironmentKind, Geometry, Mode, PageSize,
    SyncMode, RO, RW,
};
use std::{ops::Deref, path::Path};

pub mod table;
use table::{Decode, DupSort, Encode, Table};

pub mod tables;
use tables::TABLES;

pub mod cursor;

pub mod tx;
use tx::Tx;

mod error;
pub use error::KVError;

/// Environment used when opening a MDBX environment. RO/RW.
#[derive(Debug)]
pub enum EnvKind {
    /// Read-only MDBX environment.
    RO,
    /// Read-write MDBX environment.
    RW,
}

/// Wrapper for the libmdbx environment.
#[derive(Debug)]
pub struct Env<E: EnvironmentKind> {
    /// Libmdbx-sys environment.
    pub inner: Environment<E>,
}

impl<E: EnvironmentKind> Env<E> {
    /// Opens the database at the specified path with the given `EnvKind`.
    ///
    /// It does not create the tables, for that call [`create_tables`].
    pub fn open(path: &Path, kind: EnvKind) -> Result<Env<E>, KVError> {
        let mode = match kind {
            EnvKind::RO => Mode::ReadOnly,
            EnvKind::RW => Mode::ReadWrite { sync_mode: SyncMode::Durable },
        };

        let env = Env {
            inner: Environment::new()
                .set_max_dbs(TABLES.len())
                .set_geometry(Geometry {
                    size: Some(0..0x100000),     // TODO: reevaluate
                    growth_step: Some(0x100000), // TODO: reevaluate
                    shrink_threshold: None,
                    page_size: Some(PageSize::Set(default_page_size())),
                })
                .set_flags(EnvironmentFlags {
                    mode,
                    no_rdahead: true, // TODO: reevaluate
                    coalesce: true,
                    ..Default::default()
                })
                .open(path)
                .map_err(KVError::DatabaseLocation)?,
        };

        Ok(env)
    }

    /// Creates all the defined tables, if necessary.
    pub fn create_tables(&self) -> Result<(), KVError> {
        let tx = self.inner.begin_rw_txn().map_err(KVError::InitTransaction)?;

        for (table_type, table) in TABLES {
            let flags = match table_type {
                TableType::Table => DatabaseFlags::default(),
                TableType::DupSort => DatabaseFlags::DUP_SORT,
            };

            tx.create_db(Some(table), flags).map_err(KVError::TableCreation)?;
        }

        tx.commit()?;

        Ok(())
    }
}

impl<E: EnvironmentKind> Env<E> {
    /// Initiates a read-only transaction. It should be committed or rolled back in the end, so it
    /// frees up pages.
    pub fn begin_tx(&self) -> Result<Tx<'_, RO, E>, KVError> {
        Ok(Tx::new(self.inner.begin_ro_txn().map_err(KVError::InitTransaction)?))
    }

    /// Initiates a read-write transaction. It should be committed or rolled back in the end.
    pub fn begin_mut_tx(&self) -> Result<Tx<'_, RW, E>, KVError> {
        Ok(Tx::new(self.inner.begin_rw_txn().map_err(KVError::InitTransaction)?))
    }

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    pub fn view<T, F>(&self, f: F) -> Result<T, KVError>
    where
        F: Fn(&Tx<'_, RO, E>) -> T,
    {
        let tx = self.begin_tx()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }

    /// Takes a function and passes a write-read transaction into it, making sure it's committed in
    /// the end of the execution.
    pub fn update<T, F>(&self, f: F) -> Result<T, KVError>
    where
        F: Fn(&Tx<'_, RW, E>) -> T,
    {
        let tx = self.begin_mut_tx()?;

        let res = f(&tx);
        tx.commit()?;

        Ok(res)
    }
}

impl<E: EnvironmentKind> Deref for Env<E> {
    type Target = libmdbx::Environment<E>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::{tables::PlainState, Env, EnvKind};
    use libmdbx::{NoWriteMap, WriteMap};
    use reth_primitives::Address;
    use std::str::FromStr;
    use tempfile::TempDir;

    const ERROR_DB_CREATION: &str = "Not able to create the mdbx file.";
    const ERROR_DB_OPEN: &str = "Not able to open existing mdbx file.";
    const ERROR_TABLE_CREATION: &str = "Not able to create tables in the database.";
    const ERROR_PUT: &str = "Not able to insert value into table.";
    const ERROR_GET: &str = "Not able to get value from table.";
    const ERROR_COMMIT: &str = "Not able to commit transaction.";
    const ERROR_RETURN_VALUE: &str = "Mismatching result.";
    const ERROR_INIT_TX: &str = "Failed to create a MDBX transaction.";
    const ERROR_ETH_ADDRESS: &str = "Invalid address.";
    const ERROR_TEMPDIR: &str = "Not able to create a temporary directory.";

    #[test]
    fn db_creation() {
        Env::<NoWriteMap>::open(&TempDir::new().expect(ERROR_TEMPDIR).into_path(), EnvKind::RW)
            .expect(ERROR_DB_CREATION);
    }

    #[test]
    fn db_manual_put_get() {
        let env =
            Env::<NoWriteMap>::open(&TempDir::new().expect(ERROR_TEMPDIR).into_path(), EnvKind::RW)
                .expect(ERROR_DB_CREATION);
        env.create_tables().expect(ERROR_TABLE_CREATION);

        let value = vec![1, 3, 3, 7];
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        // PUT
        let tx = env.begin_mut_tx().expect(ERROR_INIT_TX);
        tx.put::<PlainState>(key, value.clone()).expect(ERROR_PUT);
        tx.commit().expect(ERROR_COMMIT);

        // GET
        let tx = env.begin_tx().expect(ERROR_INIT_TX);
        let result = tx.get::<PlainState>(key).expect(ERROR_GET);
        assert!(result.expect(ERROR_RETURN_VALUE) == value);
        tx.commit().expect(ERROR_COMMIT);
    }

    #[test]
    fn db_closure_put_get() {
        let path = TempDir::new().expect(ERROR_TEMPDIR).into_path();

        let value = vec![1, 3, 3, 7];
        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047")
            .expect(ERROR_ETH_ADDRESS);

        {
            let env = Env::<WriteMap>::open(&path, EnvKind::RW).expect(ERROR_DB_OPEN);
            env.create_tables().expect(ERROR_TABLE_CREATION);

            // PUT
            let result = env.update(|tx| {
                tx.put::<PlainState>(key, value.clone()).expect(ERROR_PUT);
                200
            });
            assert!(result.expect(ERROR_RETURN_VALUE) == 200);
        }

        let env = Env::<WriteMap>::open(&path, EnvKind::RO).expect(ERROR_DB_CREATION);

        // GET
        let result = env.view(|tx| tx.get::<PlainState>(key).expect(ERROR_GET)).expect(ERROR_GET);

        assert!(result == Some(value))
    }
}
