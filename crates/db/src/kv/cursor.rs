use crate::kv::{Decode, DupSort, Encode, Table};
use libmdbx::{self, TransactionKind};
use std::borrow::Cow;

pub struct Cursor<'tx, K: TransactionKind, T: Table> {
    pub inner: libmdbx::Cursor<'tx, K>,
    pub table: String, // todo
    pub _dbi: std::marker::PhantomData<T>,
}

#[macro_export]
macro_rules! decode {
    ($v:expr) => {
        $v?.map(decoder::<T>).transpose()
    };
}

impl<'tx, K: TransactionKind, T: Table> Cursor<'tx, K, T> {
    pub fn first(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.first())
    }

    pub fn seek(&mut self, key: T::SeekKey) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.set_range(key.encode().as_ref()))
    }

    pub fn seek_exact(&mut self, key: T::Key) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.set_key(key.encode().as_ref()))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next())
    }

    pub fn prev(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.prev())
    }

    pub fn last(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.last())
    }

    pub fn current(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.get_current())
    }

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
    pub fn next_dup(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next_dup())
    }

    pub fn next_no_dup(&mut self) -> eyre::Result<Option<(T::Key, T::Value)>>
    where
        T::Key: Decode,
    {
        decode!(self.inner.next_nodup())
    }

    pub fn next_dup_val(&mut self) -> eyre::Result<Option<T::Value>> {
        self.inner.next_dup()?.map(decode_value::<T>).transpose()
    }

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

pub fn decoder<'a, T>(kv: (Cow<'a, [u8]>, Cow<'a, [u8]>)) -> eyre::Result<(T::Key, T::Value)>
where
    T: Table,
    T::Key: Decode,
{
    Ok((Decode::decode(&kv.0)?, Decode::decode(&kv.1)?))
}

pub fn decode_value<'a, T>(kv: (Cow<'a, [u8]>, Cow<'a, [u8]>)) -> eyre::Result<T::Value>
where
    T: Table,
{
    Decode::decode(&kv.1)
}

pub fn decode_one<'a, T>(value: Cow<'a, [u8]>) -> eyre::Result<T::Value>
where
    T: Table,
{
    Decode::decode(&value)
}

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
