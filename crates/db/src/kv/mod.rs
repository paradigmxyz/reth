use crate::utils::{default_page_size, TEMP_PLACEHOLDER_NUM_TABLES};
use libmdbx::{
    Environment, EnvironmentFlags, EnvironmentKind, Error, Geometry, Mode, PageSize, SyncMode,
    WriteMap, RO, RW,
};
use std::{ops::Deref, path::Path};

pub mod table;
use table::{Decode, DupSort, Encode, Table};

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
    pub fn open(path: &Path, kind: EnvKind) -> Result<Env<WriteMap>, Error> {
        let mode = match kind {
            EnvKind::RO => Mode::ReadOnly,
            EnvKind::RW => Mode::ReadWrite { sync_mode: SyncMode::Durable },
        };

        let env = Env {
            inner: Environment::new()
                .set_max_dbs(TEMP_PLACEHOLDER_NUM_TABLES)
                .set_geometry(Geometry {
                    size: Some(0..usize::MAX),
                    growth_step: Some(isize::MAX),
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
        // loop & create
        tx.commit()?;
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
