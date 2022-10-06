use crate::utils::default_page_size;
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
                .set_max_dbs(10)
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
        Ok(Tx::new(self.inner.begin_ro_txn()?))
    }

    pub fn begin_mut_tx(&self) -> eyre::Result<Tx<'_, RW, E>> {
        Ok(Tx::new(self.inner.begin_rw_txn()?))
    }

    /// Takes a function and passes a read-only transaction into it, making sure it's closed in the
    /// end of the execution.
    pub fn view<T, F>(&self, f: F) -> eyre::Result<T>
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
    pub fn update<T, F>(&self, f: F) -> eyre::Result<T>
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
    use super::{tables::Account, Env, EnvKind};
    use libmdbx::{NoWriteMap, WriteMap};
    use reth_primitives::Address;
    use std::str::FromStr;
    use tempfile::TempDir;

    #[test]
    fn db_creation() {
        Env::<NoWriteMap>::open(&TempDir::new().unwrap().into_path(), EnvKind::RW).unwrap();
    }

    #[test]
    fn db_manual_put_get() {
        let env =
            Env::<NoWriteMap>::open(&TempDir::new().unwrap().into_path(), EnvKind::RW).unwrap();

        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047").unwrap();
        let value = vec![1, 3, 3, 7];

        // PUT
        let tx = env.begin_mut_tx().unwrap();
        tx.put(Account, key, value).unwrap();
        tx.commit().unwrap();

        // GET
        let tx = env.begin_tx().unwrap();
        let _result = tx.get(Account, key).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn db_closure_put_get() {
        let path = TempDir::new().unwrap().into_path();

        let key = Address::from_str("0xa2c122be93b0074270ebee7f6b7292c7deb45047").unwrap();
        let value = vec![1, 3, 3, 7];

        {
            let env = Env::<WriteMap>::open(&path, EnvKind::RW).unwrap();

            // PUT
            let result = env.update(|tx| {
                tx.put(Account, key, value.clone()).unwrap();
                200
            });
            assert!(result.unwrap() == 200);
        }

        let env = Env::<WriteMap>::open(&path, EnvKind::RO).unwrap();

        // GET
        let result = env.view(|tx| tx.get(Account, key).unwrap()).unwrap();

        assert!(result == Some(value))
    }
}
