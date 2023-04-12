//! Transaction wrapper for libmdbx-sys.

use super::cursor::Cursor;
use crate::{
    table::{Compress, DupSort, Encode, Table, TableImporter},
    tables::{utils::decode_one, NUM_TABLES, TABLES},
    transaction::{DbTx, DbTxGAT, DbTxMut, DbTxMutGAT},
    Error,
};
use metrics::histogram;
use parking_lot::RwLock;
use reth_libmdbx::{EnvironmentKind, Transaction, TransactionKind, WriteFlags, DBI, RW};
use std::{marker::PhantomData, sync::Arc, time::Instant};

/// Wrapper for the libmdbx transaction.
#[derive(Debug)]
pub struct Tx<'a, K: TransactionKind, E: EnvironmentKind> {
    /// Libmdbx-sys transaction.
    pub inner: Transaction<'a, K, E>,
    /// Database table handle cache
    pub db_handles: Arc<RwLock<[Option<DBI>; NUM_TABLES]>>,
}

impl<'env, K: TransactionKind, E: EnvironmentKind> Tx<'env, K, E> {
    /// Creates new `Tx` object with a `RO` or `RW` transaction.
    pub fn new<'a>(inner: Transaction<'a, K, E>) -> Self
    where
        'a: 'env,
    {
        Self { inner, db_handles: Default::default() }
    }

    /// Gets this transaction ID.
    pub fn id(&self) -> u64 {
        self.inner.id()
    }

    /// Gets a table database handle if it exists, otherwise creates it.
    pub fn get_dbi<T: Table>(&self) -> Result<DBI, Error> {
        let mut handles = self.db_handles.write();

        let table_index = TABLES
            .iter()
            .enumerate()
            .find_map(|(idx, (_, table))| (table == &T::NAME).then_some(idx))
            .expect("Requested table should be part of `TABLES`.");

        let dbi_handle = handles.get_mut(table_index).expect("should exist");
        if dbi_handle.is_none() {
            *dbi_handle = Some(
                self.inner.open_db(Some(T::NAME)).map_err(|e| Error::InitCursor(e.into()))?.dbi(),
            );
        }

        Ok(dbi_handle.expect("is some; qed"))
    }

    /// Create db Cursor
    pub fn new_cursor<T: Table>(&self) -> Result<Cursor<'env, K, T>, Error> {
        Ok(Cursor {
            inner: self
                .inner
                .cursor_with_dbi(self.get_dbi::<T>()?)
                .map_err(|e| Error::InitCursor(e.into()))?,
            table: T::NAME,
            _dbi: PhantomData,
            buf: vec![],
        })
    }
}

impl<'a, K: TransactionKind, E: EnvironmentKind> DbTxGAT<'a> for Tx<'_, K, E> {
    type Cursor<T: Table> = Cursor<'a, K, T>;
    type DupCursor<T: DupSort> = Cursor<'a, K, T>;
}

impl<'a, K: TransactionKind, E: EnvironmentKind> DbTxMutGAT<'a> for Tx<'_, K, E> {
    type CursorMut<T: Table> = Cursor<'a, RW, T>;
    type DupCursorMut<T: DupSort> = Cursor<'a, RW, T>;
}

impl<'a, E: EnvironmentKind> TableImporter<'a> for Tx<'_, RW, E> {}

impl<'tx, K: TransactionKind, E: EnvironmentKind> DbTx<'tx> for Tx<'tx, K, E> {
    // Iterate over read only values in database.
    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, Error> {
        self.new_cursor()
    }

    /// Iterate over read only values in database.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, Error> {
        self.new_cursor()
    }

    fn commit(self) -> Result<bool, Error> {
        let start = Instant::now();
        let result = self.inner.commit().map_err(|e| Error::Commit(e.into()));
        histogram!("tx.commit", start.elapsed());
        result
    }

    fn drop(self) {
        drop(self.inner)
    }

    fn get<T: Table>(&self, key: T::Key) -> Result<Option<<T as Table>::Value>, Error> {
        self.inner
            .get(self.get_dbi::<T>()?, key.encode().as_ref())
            .map_err(|e| Error::Read(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }
}

impl<E: EnvironmentKind> DbTxMut<'_> for Tx<'_, RW, E> {
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error> {
        self.inner
            .put(self.get_dbi::<T>()?, &key.encode(), &value.compress(), WriteFlags::UPSERT)
            .map_err(|e| Error::Write(e.into()))
    }

    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error> {
        let mut data = None;

        let value = value.map(Compress::compress);
        if let Some(value) = &value {
            data = Some(value.as_ref());
        };

        self.inner
            .del(self.get_dbi::<T>()?, key.encode(), data)
            .map_err(|e| Error::Delete(e.into()))
    }

    fn clear<T: Table>(&self) -> Result<(), Error> {
        self.inner.clear_db(self.get_dbi::<T>()?).map_err(|e| Error::Delete(e.into()))?;

        Ok(())
    }

    fn cursor_write<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error> {
        self.new_cursor()
    }

    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error> {
        self.new_cursor()
    }
}
