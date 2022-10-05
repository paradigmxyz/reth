use crate::kv::{
    cursor::{decode_one, Cursor},
    table::{Encode, Table},
};
use libmdbx::{EnvironmentKind, Transaction, TransactionKind, WriteFlags, RW};
use std::marker::PhantomData;

pub struct Tx<'a, K: TransactionKind, E: EnvironmentKind> {
    pub inner: Transaction<'a, K, E>,
}

impl<'env, K: TransactionKind, E: EnvironmentKind> Tx<'env, K, E> {
    pub fn new<'a>(inner: Transaction<'a, K, E>) -> Self
    where
        'a: 'env,
    {
        Self { inner }
    }

    pub fn id(&self) -> u64 {
        self.inner.id()
    }

    pub fn cursor<'a, T: Table>(&'a self, table: T) -> eyre::Result<Cursor<'a, K, T>>
    where
        'env: 'a,
        T: Table,
    {
        let table_name = table.db_name();

        Ok(Cursor {
            inner: self.inner.cursor(&self.inner.open_db(Some(table_name))?)?,
            table: table_name.to_string(), // todo
            _dbi: PhantomData,
        })
    }

    pub fn get<T: Table>(&self, table: T, key: T::Key) -> eyre::Result<Option<T::Value>> {
        self.inner
            .get(&self.inner.open_db(Some(table.db_name()))?, key.encode().as_ref())?
            .map(decode_one::<T>)
            .transpose()
    }
}

impl<'a, E: EnvironmentKind> Tx<'a, RW, E> {
    pub fn put<T>(&self, table: T, k: T::Key, v: T::Value) -> eyre::Result<()>
    where
        T: Table,
    {
        Ok(self.inner.put(
            &self.inner.open_db(Some(table.db_name()))?,
            &k.encode(),
            &v.encode(),
            WriteFlags::UPSERT,
        )?)
    }

    pub fn delete<T>(&self, table: T, key: T::Key, value: Option<T::Value>) -> eyre::Result<bool>
    where
        T: Table,
    {
        let mut data = None;

        let value = value.map(Encode::encode);
        if let Some(value) = &value {
            data = Some(value.as_ref());
        };

        Ok(self.inner.del(&self.inner.open_db(Some(table.db_name()))?, key.encode(), data)?)
    }

    pub fn clear<T>(&self, table: T) -> eyre::Result<()>
    where
        T: Table,
    {
        self.inner.clear_db(&self.inner.open_db(Some(table.db_name()))?)?;

        Ok(())
    }

    pub fn commit(self) -> eyre::Result<bool> {
        self.inner.commit().map_err(From::from)
    }
}
