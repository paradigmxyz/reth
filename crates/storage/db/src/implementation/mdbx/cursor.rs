//! Cursor wrapper for libmdbx-sys.

use std::{borrow::Cow, marker::PhantomData};

use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, Walker},
    table::{Compress, DupSort, Encode, Table},
    tables::utils::*,
    Error,
};
use reth_libmdbx::{self, TransactionKind, WriteFlags, RO, RW};

/// Alias type for a `(key, value)` result coming from a cursor.
pub type PairResult<T> = Result<Option<(<T as Table>::Key, <T as Table>::Value)>, Error>;
/// Alias type for a `(key, value)` result coming from an iterator.
pub type IterPairResult<T> = Option<Result<(<T as Table>::Key, <T as Table>::Value), Error>>;
/// Alias type for a value result coming from a cursor without its key.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, Error>;

/// Read only Cursor.
pub type CursorRO<'tx, T> = Cursor<'tx, RO, T>;
/// Read write cursor.
pub type CursorRW<'tx, T> = Cursor<'tx, RW, T>;

/// Cursor wrapper to access KV items.
#[derive(Debug)]
pub struct Cursor<'tx, K: TransactionKind, T: Table> {
    /// Inner `libmdbx` cursor.
    pub inner: reth_libmdbx::Cursor<'tx, K>,
    /// Table name as is inside the database.
    pub table: &'static str,
    /// Phantom data to enforce encoding/decoding.
    pub _dbi: std::marker::PhantomData<T>,
}

/// Takes `(key, value)` from the database and decodes it appropriately.
#[macro_export]
macro_rules! decode {
    ($v:expr) => {
        $v.map_err(|e| Error::Read(e.into()))?.map(decoder::<T>).transpose()
    };
}

impl<'tx, K: TransactionKind, T: Table> DbCursorRO<'tx, T> for Cursor<'tx, K, T> {
    fn first(&mut self) -> PairResult<T> {
        decode!(self.inner.first())
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        decode!(self.inner.set_key(key.encode().as_ref()))
    }

    fn next(&mut self) -> PairResult<T> {
        decode!(self.inner.next())
    }

    fn prev(&mut self) -> PairResult<T> {
        decode!(self.inner.prev())
    }

    fn last(&mut self) -> PairResult<T> {
        decode!(self.inner.last())
    }

    fn current(&mut self) -> PairResult<T> {
        decode!(self.inner.get_current())
    }

    fn walk<'cursor>(
        &'cursor mut self,
        start_key: T::Key,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized,
    {
        let start = self
            .inner
            .set_range(start_key.encode().as_ref())
            .map_err(|e| Error::Read(e.into()))?
            .map(decoder::<T>);

        Ok(Walker::new(self, start))
    }
}

impl<'tx, K: TransactionKind, T: DupSort> DbDupCursorRO<'tx, T> for Cursor<'tx, K, T> {
    fn seek(&mut self, key: <T as DupSort>::SubKey) -> PairResult<T> {
        decode!(self.inner.set_range(key.encode().as_ref()))
    }

    /// Returns the next `(key, value)` pair of a DUPSORT table.
    fn next_dup(&mut self) -> PairResult<T> {
        decode!(self.inner.next_dup())
    }

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T> {
        decode!(self.inner.next_nodup())
    }

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        self.inner.next_dup().map_err(|e| Error::Read(e.into()))?.map(decode_value::<T>).transpose()
    }

    fn seek_by_key_subkey(
        &mut self,
        key: <T as Table>::Key,
        subkey: <T as DupSort>::SubKey,
    ) -> ValueOnlyResult<T> {
        self.inner
            .get_both_range(key.encode().as_ref(), subkey.encode().as_ref())
            .map_err(|e| Error::Read(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }

    /// Returns an iterator starting at a key greater or equal than `start_key` of a DUPSORT table.
    fn walk_dup<'cursor>(
        &'cursor mut self,
        key: T::Key,
        subkey: T::SubKey,
    ) -> Result<DupWalker<'cursor, 'tx, T, Self>, Error> {
        // encode key and decode it after.
        let key = key.encode().as_ref().to_vec();

        let start = self
            .inner
            .get_both_range(key.as_ref(), subkey.encode().as_ref())
            .map_err(|e| Error::Read(e.into()))?
            .map(|val| decoder::<T>((Cow::Owned(key), val)));

        Ok(DupWalker::<'cursor, 'tx, T, Self> { cursor: self, start, _tx_phantom: PhantomData {} })
    }
}

impl<'tx, T: Table> DbCursorRW<'tx, T> for Cursor<'tx, RW, T> {
    /// Database operation that will update an existing row if a specified value already
    /// exists in a table, and insert a new row if the specified value doesn't already exist
    fn upsert(&mut self, key: T::Key, value: T::Value) -> Result<(), Error> {
        // Default `WriteFlags` is UPSERT
        self.inner
            .put(key.encode().as_ref(), value.compress().as_ref(), WriteFlags::UPSERT)
            .map_err(|e| Error::Write(e.into()))
    }

    fn insert(&mut self, key: T::Key, value: T::Value) -> Result<(), Error> {
        self.inner
            .put(key.encode().as_ref(), value.compress().as_ref(), WriteFlags::NO_OVERWRITE)
            .map_err(|e| Error::Write(e.into()))
    }

    /// Appends the data to the end of the table. Consequently, the append operation
    /// will fail if the inserted key is less than the last table key
    fn append(&mut self, key: T::Key, value: T::Value) -> Result<(), Error> {
        self.inner
            .put(key.encode().as_ref(), value.compress().as_ref(), WriteFlags::APPEND)
            .map_err(|e| Error::Write(e.into()))
    }

    fn delete_current(&mut self) -> Result<(), Error> {
        self.inner.del(WriteFlags::CURRENT).map_err(|e| Error::Delete(e.into()))
    }
}

impl<'tx, T: DupSort> DbDupCursorRW<'tx, T> for Cursor<'tx, RW, T> {
    fn delete_current_duplicates(&mut self) -> Result<(), Error> {
        self.inner.del(WriteFlags::NO_DUP_DATA).map_err(|e| Error::Delete(e.into()))
    }

    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), Error> {
        self.inner
            .put(key.encode().as_ref(), value.compress().as_ref(), WriteFlags::APPEND_DUP)
            .map_err(|e| Error::Write(e.into()))
    }
}
