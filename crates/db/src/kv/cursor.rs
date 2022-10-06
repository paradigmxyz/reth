use crate::{
    kv::{Decode, DupSort, Encode, Table},
    utils::*,
};
use libmdbx::{self, TransactionKind};

// Cursor wrapper to access KV items.
pub struct Cursor<'tx, K: TransactionKind, T: Table> {
    /// Inner `libmdbx` cursor.
    pub inner: libmdbx::Cursor<'tx, K>,
    /// Table name.
    pub table: String, // TODO
    pub _dbi: std::marker::PhantomData<T>,
}

#[macro_export]
macro_rules! decode {
    ($v:expr) => {
        $v?.map(decoder::<T>).transpose()
    };
}

impl<'tx, K: TransactionKind, T: Table> Cursor<'tx, K, T> {
    /// Returns the first `(key, value)` pair.
    pub fn first(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.first())
    }

    /// Seeks for a `(key, value)` pair greater or equal than `key`.
    pub fn seek(&mut self, key: T::SeekKey) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.set_range(key.encode().as_ref()))
    }

    /// Seeks for the exact `(key, value)` pair with `key`.
    pub fn seek_exact(&mut self, key: T::Key) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.set_key(key.encode().as_ref()))
    }

    /// Returns the next `(key, value)` pair.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next())
    }

    /// Returns the previous `(key, value)` pair.
    pub fn prev(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.prev())
    }

    /// Returns the last `(key, value)` pair.
    pub fn last(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.last())
    }

    /// Returns the current `(key, value)` pair of the cursor.
    pub fn current(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.get_current())
    }

    /// Returns an iterator starting at a key greater or equal than `start_key`.
    pub fn walk(
        mut self,
        start_key: T::Key,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<(<T as Table>::Key, <T as Table>::Value)>>>
    where
        T::Key: Decode,
    {
        let start = self.inner.set_range(start_key.encode().as_ref())?.map(decoder::<T>);

        Ok(Walker { cursor: self, start })
    }
}

impl<'txn, K, T> Cursor<'txn, K, T>
where
    K: TransactionKind,
    T: DupSort,
{
    /// Returns the next `(key, value)` pair of a DUPSORT table.
    pub fn next_dup(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next_dup())
    }

    /// Returns the next `(key, value)` pair skipping the duplicates.
    pub fn next_no_dup(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next_nodup())
    }

    /// Returns the next `value` of a duplicate `key`.
    pub fn next_dup_val(&mut self) -> eyre::Result<Option<T::Value>> {
        self.inner.next_dup()?.map(decode_value::<T>).transpose()
    }

    /// Returns an iterator starting at a key greater or equal than `start_key` of a DUPSORT table.
    pub fn walk_dup(
        mut self,
        key: T::Key,
        subkey: T::SubKey,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<<T as Table>::Value>>> {
        let start = self
            .inner
            .get_both_range(key.encode().as_ref(), subkey.encode().as_ref())?
            .map(decode_one::<T>);

        Ok(DupWalker { cursor: self, start })
    }
}

/// Provides an iterator to `Cursor` when handling `Table`.
pub struct Walker<'a, K: TransactionKind, T: Table> {
    pub cursor: Cursor<'a, K, T>,
    pub start: Option<eyre::Result<(T::Key, T::Value)>>,
}

impl<'tx, K: TransactionKind, T: Table> std::iter::Iterator for Walker<'tx, K, T>
where
    T::Key: Decode,
{
    type Item = eyre::Result<(T::Key, T::Value)>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }

        self.cursor.next().transpose()
    }
}

/// Provides an iterator to `Cursor` when handling a `DupSort` table.
pub struct DupWalker<'a, K: TransactionKind, T: DupSort> {
    pub cursor: Cursor<'a, K, T>,
    pub start: Option<eyre::Result<T::Value>>,
}

impl<'tx, K: TransactionKind, T: DupSort> std::iter::Iterator for DupWalker<'tx, K, T> {
    type Item = eyre::Result<T::Value>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.start.take();
        if start.is_some() {
            return start
        }
        self.cursor.next_dup_val().transpose()
    }
}
