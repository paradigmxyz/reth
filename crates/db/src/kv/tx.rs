//! Transaction wrapper for libmdbx-sys.

use crate::{kv::cursor::Cursor, utils::decode_one};
use libmdbx::{EnvironmentKind, Transaction, TransactionKind, WriteFlags, RW};
use reth_interfaces::db::{DbTx, DbTxMut, DupSort, Encode, Error, Table};
use std::marker::PhantomData;

/// Wrapper for the libmdbx transaction.
#[derive(Debug)]
pub struct Tx<'a, K: TransactionKind, E: EnvironmentKind> {
    /// Libmdbx-sys transaction.
    pub inner: Transaction<'a, K, E>,
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

    /// Create db Cursor
    pub fn new_cursor<T: Table>(&self) -> Result<Cursor<'env, K, T>, Error> {
        Ok(Cursor {
            inner: self
                .inner
                .cursor(&self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?)
                .map_err(|e| Error::Internal(e.into()))?,
            table: T::NAME,
            _dbi: PhantomData,
        })
    }
}

impl<'env, K: TransactionKind, E: EnvironmentKind> DbTx<'env> for Tx<'env, K, E> {
    /// Cursor GAT
    type Cursor<T: Table> = Cursor<'env, K, T>;
    /// DupCursor GAT
    type DupCursor<T: DupSort> = Cursor<'env, K, T>;
    /// Iterate over read only values in database.
    fn cursor<T: Table>(&self) -> Result<Self::Cursor<T>, Error> {
        self.new_cursor()
    }

    /// Iterate over read only values in database.
    fn cursor_dup<T: DupSort>(&self) -> Result<Self::DupCursor<T>, Error> {
        self.new_cursor()
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

    fn clear<T: Table>(&self) -> Result<(), Error> {
        self.inner
            .clear_db(&self.inner.open_db(Some(T::NAME)).map_err(|e| Error::Internal(e.into()))?)
            .map_err(|e| Error::Internal(e.into()))?;

        Ok(())
    }

    fn cursor_mut<T: Table>(&self) -> Result<Self::CursorMut<T>, Error> {
        self.new_cursor()
    }

    fn cursor_dup_mut<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, Error> {
        self.new_cursor()
    }
}
