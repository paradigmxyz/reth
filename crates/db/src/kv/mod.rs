use crate::utils::{default_page_size, TEMP_PLACEHOLDER_NUM_TABLES};
use libmdbx::{
    DatabaseFlags, Environment, EnvironmentFlags, EnvironmentKind, Error, Geometry, Mode, PageSize,
    SyncMode, RO, RW,
};
use std::{ops::Deref, path::Path};

pub mod table;
use table::{Decode, DupSort, Encode, Table};

pub mod tables;
use tables::Account;

pub mod cursor;

pub mod tx;
use tx::Tx;

pub enum EnvKind {
    RO,
    RW,
}

pub struct Env<E: EnvironmentKind> {
    pub inner: Environment<E>,
}

impl<E: EnvironmentKind> Env<E> {
    pub fn open(path: &Path, kind: EnvKind) -> Result<Env<E>, Error> {
        let mode = match kind {
            EnvKind::RO => Mode::ReadOnly,
            EnvKind::RW => Mode::ReadWrite { sync_mode: SyncMode::Durable },
        };

        let env = Env {
            inner: Environment::new()
                .set_max_dbs(TEMP_PLACEHOLDER_NUM_TABLES)
                .set_geometry(Geometry {
                    size: Some(0..0x100000),     // TODO
                    growth_step: Some(0x100000), // TODO
                    shrink_threshold: None,
                    page_size: Some(PageSize::Set(default_page_size())),
                })
                .set_flags(EnvironmentFlags {
                    mode,
                    no_rdahead: true,
                    coalesce: true,
                    ..Default::default()
                })
                .open(path)?,
        };

        if let EnvKind::RW = kind {
            env.maybe_create_tables()?;
        }

        Ok(env)
    }

    fn maybe_create_tables(&self) -> Result<(), Error> {
        let tx = self.inner.begin_rw_txn()?;
        // TODO: loop & create dup_sort flag
        tx.create_db(Some(Account::name()), DatabaseFlags::default())?;
        // tx.create_db(Some(Storage::name()), DatabaseFlags::default())?;

        tx.commit()?;

        //         let tx = s.inner.begin_rw_txn()?;
        // for (table, info) in chart {
        //     tx.create_db(
        //         Some(table),
        //         if info.dup_sort {
        //             DatabaseFlags::DUP_SORT
        //         } else {
        //             DatabaseFlags::default()
        //         },
        //     )?;
        // }
        // tx.commit()?;
        Ok(())
    }
}

impl<E: EnvironmentKind> Env<E> {
    pub fn begin_tx(&self) -> eyre::Result<Tx<'_, RO, E>> {
        Ok(Tx { inner: self.inner.begin_ro_txn()? })
    }

    pub fn begin_mut_tx(&self) -> eyre::Result<Tx<'_, RW, E>> {
        Ok(Tx { inner: self.inner.begin_rw_txn()? })
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
    use super::{tables::Account, Env, EnvKind};
    use libmdbx::NoWriteMap;
    use reth_primitives::Address;
    use std::str::FromStr;
    use tempfile::TempDir;

    #[test]
    fn db_creation() {
        Env::<NoWriteMap>::open(&TempDir::new().unwrap().into_path(), EnvKind::RW).unwrap();
    }

    #[test]
    fn db_put_get() {
        let env =
            Env::<NoWriteMap>::open(&TempDir::new().unwrap().into_path(), EnvKind::RW).unwrap();

        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047").unwrap();
        let value = vec![1, 3, 3, 7];

        let tx = env.begin_mut_tx().unwrap();
        tx.put(Account, key, value.clone()).unwrap();
        tx.commit().unwrap();

        let tx = env.begin_tx().unwrap();
        let result = tx.get(Account, key).unwrap();
        tx.commit().unwrap();

        assert!(result == Some(value))
    }
}
