//! Cursor wrapper for libmdbx-sys.

use super::error::KVError;
use crate::{
    kv::{Decode, DupSort, Encode, Table},
    utils::*,
};
use libmdbx::{self, TransactionKind, WriteFlags, RW};

/// Alias type for a `(key, value)` result coming from a cursor.
pub type PairResult<T> = Result<Option<(<T as Table>::Key, <T as Table>::Value)>, KVError>;
/// Alias type for a `(key, value)` result coming from an iterator.
pub type IterPairResult<T> = Option<Result<(<T as Table>::Key, <T as Table>::Value), KVError>>;
/// Alias type for a value result coming from a cursor without its key.
pub type ValueOnlyResult<T> = Result<Option<<T as Table>::Value>, KVError>;

/// Cursor wrapper to access KV items.
#[derive(Debug)]
pub struct Cursor<'tx, K: TransactionKind, T: Table> {
    /// Inner `libmdbx` cursor.
    pub inner: libmdbx::Cursor<'tx, K>,
    /// Table name as is inside the database.
    pub table: &'static str,
    /// Phantom data to enforce encoding/decoding.
    pub _dbi: std::marker::PhantomData<T>,
}

/// Takes `(key, value)` from the database and decodes it appropriately.
#[macro_export]
macro_rules! decode {
    ($v:expr) => {
        $v?.map(decoder::<T>).transpose()
    };
}

impl<'tx, K: TransactionKind, T: Table> Cursor<'tx, K, T> {
    /// Returns the first `(key, value)` pair.
    pub fn first(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.first())
    }

    /// Seeks for a `(key, value)` pair greater or equal than `key`.
    pub fn seek(&mut self, key: T::SeekKey) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.set_range(key.encode().as_ref()))
    }

    /// Seeks for the exact `(key, value)` pair with `key`.
    pub fn seek_exact(&mut self, key: T::Key) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.set_key(key.encode().as_ref()))
    }

    /// Returns the next `(key, value)` pair.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next())
    }

    /// Returns the previous `(key, value)` pair.
    pub fn prev(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.prev())
    }

    /// Returns the last `(key, value)` pair.
    pub fn last(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.last())
    }

    /// Returns the current `(key, value)` pair of the cursor.
    pub fn current(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.get_current())
    }

    /// Returns an iterator starting at a key greater or equal than `start_key`.
    pub fn walk(
        mut self,
        start_key: T::Key,
    ) -> Result<
        impl Iterator<Item = Result<(<T as Table>::Key, <T as Table>::Value), KVError>>,
        KVError,
    >
    where
        T::Key: Decode,
    {
        let start = self.inner.set_range(start_key.encode().as_ref())?.map(decoder::<T>);

        Ok(Walker { cursor: self, start })
    }
}

impl<'tx, T: Table> Cursor<'tx, RW, T> {
    /// Inserts a `(key, value)` to the database. Repositions the cursor to the new item
    pub fn put(&mut self, k: T::Key, v: T::Value, f: Option<WriteFlags>) -> Result<(), KVError> {
        self.inner
            .put(k.encode().as_ref(), v.encode().as_ref(), f.unwrap_or_default())
            .map_err(KVError::Put)
    }
}

impl<'txn, K, T> Cursor<'txn, K, T>
where
    K: TransactionKind,
    T: DupSort,
{
    /// Returns the next `(key, value)` pair of a DUPSORT table.
    pub fn next_dup(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next_dup())
    }

    /// Returns the next `(key, value)` pair skipping the duplicates.
    pub fn next_no_dup(&mut self) -> PairResult<T>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next_nodup())
    }

    /// Returns the next `value` of a duplicate `key`.
    pub fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        self.inner.next_dup()?.map(decode_value::<T>).transpose()
    }

    /// Returns an iterator starting at a key greater or equal than `start_key` of a DUPSORT table.
    pub fn walk_dup(
        mut self,
        key: T::Key,
        subkey: T::SubKey,
    ) -> Result<impl Iterator<Item = Result<<T as Table>::Value, KVError>>, KVError> {
        let start = self
            .inner
            .get_both_range(key.encode().as_ref(), subkey.encode().as_ref())?
            .map(decode_one::<T>);

        Ok(DupWalker { cursor: self, start })
    }
}

/// Provides an iterator to `Cursor` when handling `Table`.
#[derive(Debug)]
pub struct Walker<'a, K: TransactionKind, T: Table> {
    /// Cursor to be used to walk through the table.
    pub cursor: Cursor<'a, K, T>,
    /// `(key, value)` where to start the walk.
    pub start: IterPairResult<T>,
}

impl<'tx, K: TransactionKind, T: Table> std::iter::Iterator for Walker<'tx, K, T>
where
    T::Key: Decode,
{
    type Item = Result<(T::Key, T::Value), KVError>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }

        self.cursor.next().transpose()
    }
}

/// Provides an iterator to `Cursor` when handling a `DupSort` table.
#[derive(Debug)]
pub struct DupWalker<'a, K: TransactionKind, T: DupSort> {
    /// Cursor to be used to walk through the table.
    pub cursor: Cursor<'a, K, T>,
    /// Value where to start the walk.
    pub start: Option<Result<T::Value, KVError>>,
}

impl<'tx, K: TransactionKind, T: DupSort> std::iter::Iterator for DupWalker<'tx, K, T> {
    type Item = Result<T::Value, KVError>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }
        self.cursor.next_dup_val().transpose()
    }
}
