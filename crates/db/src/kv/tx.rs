//! Transaction wrapper for libmdbx-sys.

use crate::{
    kv::cursor::{Cursor, ValueOnlyResult},
    utils::decode_one,
};
use libmdbx::{EnvironmentKind, Transaction, TransactionKind, WriteFlags, RW};
use reth_interfaces::db::{
    DbTx, DbTxMut, DupSort, Encode, Error, Table,
};
use std::marker::PhantomData;

/// Wrapper for the libmdbx transaction.
#[derive(Debug)]
pub struct Tx<'a, K: TransactionKind, E: EnvironmentKind> {
    /// Libmdbx-sys transaction.
    pub inner: Transaction<'a, K, E>,
}

impl<'env, K: TransactionKind, E: EnvironmentKind> DbTx<'env> for Tx<'env, K, E> {
    /// Cursor GAT
    type Cursor<T: Table> = Cursor<'env, K, T>;
    /// DupCursor GAT
    type DupCursor<T: DupSort> = Cursor<'env, K, T>;
    /// Iterate over read only values in database.
    fn cursor<T: Table>(&self) -> Result<Self::Cursor<T>, Error> {
        Ok(Cursor {
            inner: self
                .inner
                .cursor(&self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?)
                .map_err(|e| Error::Internal(e.into()))?,
            table: T::NAME,
            _dbi: PhantomData,
        })
    }

    /// Iterate over read only values in database.
    fn cursor_dup<T: DupSort>(&self) -> Result<Self::DupCursor<T>, Error> {
        // NOTE: it is same as cursor
        Ok(Cursor {
            inner: self
                .inner
                .cursor(&self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?)
                .map_err(|e| Error::Internal(e.into()))?,
            table: T::NAME,
            _dbi: PhantomData,
        })
    }

    fn commit(self) -> Result<bool, Error> {
        self.inner.commit().map_err(|e| Error::Internal(e.into()))
    }

    fn get<T: Table>(&self, key: T::Key) -> Result<Option<<T as Table>::Value>, Error> {
        self.inner
            .get(
                &self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?,
                key.encode().as_ref(),
            )
            .map_err(|e| Error::Internal(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }
}

impl<'env, E: EnvironmentKind> DbTxMut<'env> for Tx<'env, RW, E> {
    type CursorMut<T: Table> = Cursor<'env, RW, T>;

    type DupCursorMut<T: DupSort> = Cursor<'env, RW, T>;

    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error> {
        self.inner
            .put(
                &self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?,
                &key.encode(),
                &value.encode(),
                WriteFlags::UPSERT,
            )
            .map_err(|e| Error::Internal(e.into()))
    }

    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error> {
        let mut data = None;

        let value = value.map(Encode::encode);
        if let Some(value) = &value {
            data = Some(value.as_ref());
        };

        self.inner
            .del(
                &self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?,
                key.encode(),
                data,
            )
            .map_err(|e| Error::Internal(e.into()))
    }


    fn cursor_mut<T: Table>(&self) -> Result<Self::CursorMut<T>, Error> {
        todo!()
    }

    fn cursor_dup_mut<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, Error> {
        todo!()
    }

    // fn cursor_mut(&self) -> Cursor<'env, RW, T> {
    //     todo!()
    // }
}

impl<'env, K: TransactionKind, E: EnvironmentKind> Tx<'env, K, E> {
    /// Creates new `Tx` object with a `RO` or `RW` transaction.
    pub fn new<'a>(inner: Transaction<'a, K, E>) -> Self
    where
        'a: 'env,
    {
        Self { inner }
    }

    /// Gets this transaction ID.
    pub fn id(&self) -> u64 {
        self.inner.id()
    }

    /// Open cursor on `table`.
    pub fn cursor<'a, T: Table>(&'a self) -> Result<Cursor<'a, K, T>, Error>
    where
        'env: 'a,
        T: Table,
    {
        Ok(Cursor {
            inner: self
                .inner
                .cursor(&self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?)
                .map_err(|e| Error::Internal(e.into()))?,
            table: T::NAME,
            _dbi: PhantomData,
        })
    }

    /// Gets value associated with `key` on `table`. If it's a DUPSORT table, then returns the first
    /// entry.
    pub fn get<T: Table>(&self, key: T::Key) -> ValueOnlyResult<T> {
        self.inner
            .get(
                &self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?,
                key.encode().as_ref(),
            )
            .map_err(|e| Error::Internal(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }

    /// Saves all changes and frees up storage memory.
    pub fn commit(self) -> Result<bool, Error> {
        self.inner.commit().map_err(|e| Error::Internal(e.into()))
    }
}

impl<'a, E: EnvironmentKind> Tx<'a, RW, E> {
    /// Opens `table` and inserts `(key, value)` pair. If the `key` already exists, it replaces the
    /// value it if the table doesn't support DUPSORT.
    pub fn put<T>(&self, k: T::Key, v: T::Value) -> Result<(), Error>
    where
        T: Table,
    {
        self.inner
            .put(
                &self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?,
                &k.encode(),
                &v.encode(),
                WriteFlags::UPSERT,
            )
            .map_err(|e| Error::Internal(e.into()))
    }

    /// Deletes the `(key, value)` entry on `table`. When `value` is `None`, all entries with `key`
    /// are to be deleted. Otherwise, only the item matching that data shall be.
    pub fn delete<T>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error>
    where
        T: Table,
    {
        let mut data = None;

        let value = value.map(Encode::encode);
        if let Some(value) = &value {
            data = Some(value.as_ref());
        };

        self.inner
            .del(
                &self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?,
                key.encode(),
                data,
            )
            .map_err(|e| Error::Internal(e.into()))
    }

    /// Empties `table`.
    pub fn clear<T>(&self) -> Result<(), Error>
    where
        T: Table,
    {
        self.inner
            .clear_db(&self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?)
            .map_err(|e| Error::Internal(e.into()))?;

        Ok(())
    }
}
