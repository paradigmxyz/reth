//! Transaction wrapper for libmdbx-sys.

use crate::{
    kv::{
        cursor::{Cursor, ValueOnlyResult},
        table::{Encode, Table},
        KVError,
    },
    utils::decode_one,
};
use libmdbx::{EnvironmentKind, Transaction, TransactionKind, WriteFlags, RW};
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

    /// Open cursor on `table`.
    pub fn cursor<'a, T: Table>(&'a self) -> Result<Cursor<'a, K, T>, KVError>
    where
        'env: 'a,
        T: Table,
    {
        Ok(Cursor {
            inner: self.inner.cursor(&self.inner.open_db(Some(T::NAME))?)?,
            table: T::NAME,
            _dbi: PhantomData,
        })
    }

    /// Gets value associated with `key` on `table`. If it's a DUPSORT table, then returns the first
    /// entry.
    pub fn get<T: Table>(&self, key: T::Key) -> ValueOnlyResult<T> {
        self.inner
            .get(&self.inner.open_db(Some(T::NAME))?, key.encode().as_ref())?
            .map(decode_one::<T>)
            .transpose()
    }

    /// Saves all changes and frees up storage memory.
    pub fn commit(self) -> Result<bool, KVError> {
        self.inner.commit().map_err(KVError::Commit)
    }
}

impl<'a, E: EnvironmentKind> Tx<'a, RW, E> {
    /// Opens `table` and inserts `(key, value)` pair. If the `key` already exists, it replaces the
    /// value it if the table doesn't support DUPSORT.
    pub fn put<T>(&self, k: T::Key, v: T::Value) -> Result<(), KVError>
    where
        T: Table,
    {
        self.inner
            .put(&self.inner.open_db(Some(T::NAME))?, &k.encode(), &v.encode(), WriteFlags::UPSERT)
            .map_err(KVError::Put)
    }

    /// Deletes the `(key, value)` entry on `table`. When `value` is `None`, all entries with `key`
    /// are to be deleted. Otherwise, only the item matching that data shall be.
    pub fn delete<T>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, KVError>
    where
        T: Table,
    {
        let mut data = None;

        let value = value.map(Encode::encode);
        if let Some(value) = &value {
            data = Some(value.as_ref());
        };

        self.inner
            .del(&self.inner.open_db(Some(T::NAME))?, key.encode(), data)
            .map_err(KVError::Delete)
    }

    /// Empties `table`.
    pub fn clear<T>(&self) -> Result<(), KVError>
    where
        T: Table,
    {
        self.inner.clear_db(&self.inner.open_db(Some(T::NAME))?)?;

        Ok(())
    }
}
