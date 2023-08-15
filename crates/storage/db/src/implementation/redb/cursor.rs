use crate::{
    common::{KeyValue, PairResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, RangeWalker, ReverseWalker, Walker,
    },
    redb::{decode_redb_item, map_storage_error, Data},
    table::{Compress, Decompress, DupSort, Encode, Table},
};
use bytes::Bytes;
use futures::TryStreamExt;
use redb::{MultimapTable, ReadableTable};
use reth_interfaces::db::DatabaseError;
use std::{marker::PhantomData, ops::Bound};

pub struct CursorRW<'tx, T: Table> {
    pub(crate) table: redb::Table<'tx, 'tx, Data, Data>,
    pub(crate) current: Option<KeyValue<T>>,
}

impl<'tx, T: Table> DbCursorRO<'tx, T> for CursorRW<'tx, T> {
    fn first(&mut self) -> PairResult<T> {
        let item = self
            .table
            .iter()
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        if let Some(value) =
            self.table.get(Data::from(key.clone().encode())).map_err(map_storage_error)?
        {
            let pair =
                (key, <<T as Table>::Value as Decompress>::decompress(value.value().0.as_ref())?);
            self.current = Some(pair);
        } else {
            self.current = None;
        }

        Ok(self.current.clone())
    }

    fn seek(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        let item = self
            .table
            .range(Data::from(key.encode())..)
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn next(&mut self) -> PairResult<T> {
        if self.current.is_none() {
            return self.first()
        }

        let item = self
            .table
            .range(Data::from(self.current.clone().unwrap().0.encode())..)
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn prev(&mut self) -> PairResult<T> {
        if self.current.is_none() {
            // todo: should this be None instead?
            return self.first()
        }

        let item = self
            .table
            .range(..Data::from(self.current.clone().unwrap().0.encode()))
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn last(&mut self) -> PairResult<T> {
        let item = self
            .table
            .iter()
            .map_err(map_storage_error)?
            .next_back()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn current(&mut self) -> PairResult<T> {
        Ok(self.current.clone())
    }

    fn walk<'cursor>(
        &'cursor mut self,
        start_key: Option<<T as Table>::Key>,
    ) -> Result<crate::cursor::Walker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        let start =
            if let Some(start_key) = start_key { self.seek(start_key) } else { self.first() }
                .transpose();
        Ok(Walker::new(self, start))
    }

    fn walk_range<'cursor>(
        &'cursor mut self,
        range: impl std::ops::RangeBounds<<T as Table>::Key>,
    ) -> Result<crate::cursor::RangeWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        let start = match range.start_bound().cloned() {
            Bound::Included(key) => self.seek(key),
            Bound::Excluded(_) => {
                unreachable!("rust doesn't allow for Bound::Excluded in starting bounds")
            }
            Bound::Unbounded => self.first(),
        }
        .transpose();

        Ok(RangeWalker::new(self, start, range.end_bound().cloned()))
    }

    fn walk_back<'cursor>(
        &'cursor mut self,
        start_key: Option<<T as Table>::Key>,
    ) -> Result<ReverseWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        let start =
            if let Some(start_key) = start_key { self.seek(start_key) } else { self.last() }
                .transpose();

        Ok(ReverseWalker::new(self, start))
    }
}

impl<'tx, T: Table> DbCursorRW<'tx, T> for CursorRW<'tx, T> {
    fn upsert(
        &mut self,
        key: <T as Table>::Key,
        value: <T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        let key = Data::from(key.encode().as_ref());
        let value = Data::from(value.compress().as_ref());
        self.table.insert(key, value).map_err(map_storage_error)?;
        Ok(())
    }

    fn insert(
        &mut self,
        key: <T as Table>::Key,
        value: <T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        let key = Data::from(key.encode().as_ref());
        let value = Data::from(value.compress().as_ref());

        if let Some(old_value) = self.table.insert(key, value).map_err(map_storage_error)? {
            // todo return err
            todo!()
        }

        Ok(())
    }

    // todo: this optimization doesn't exist in redb
    fn append(
        &mut self,
        key: <T as Table>::Key,
        value: <T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        self.insert(key, value)
    }

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        if let Some((key, _)) = self.current.as_ref() {
            self.table.remove(Data::from(key.clone().encode())).map_err(map_storage_error)?;
        }
        Ok(())
    }
}

pub struct CursorRO<M: ReadableTable<Data, Data>, T: Table> {
    pub(crate) table: M,
    pub(crate) current: Option<KeyValue<T>>,
}

impl<'tx, M: ReadableTable<Data, Data>, T: Table> DbCursorRO<'tx, T> for CursorRO<M, T> {
    fn first(&mut self) -> PairResult<T> {
        let item = self
            .table
            .iter()
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        if let Some(value) =
            self.table.get(Data::from(key.clone().encode())).map_err(map_storage_error)?
        {
            let pair =
                (key, <<T as Table>::Value as Decompress>::decompress(value.value().0.as_ref())?);
            self.current = Some(pair);
        } else {
            self.current = None;
        }

        Ok(self.current.clone())
    }

    fn seek(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        let item = self
            .table
            .range(Data::from(key.encode())..)
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn next(&mut self) -> PairResult<T> {
        if self.current.is_none() {
            return self.first()
        }

        let item = self
            .table
            .range(Data::from(self.current.clone().unwrap().0.encode())..)
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn prev(&mut self) -> PairResult<T> {
        if self.current.is_none() {
            // todo: should this be None instead?
            return self.first()
        }

        let item = self
            .table
            .range(..Data::from(self.current.clone().unwrap().0.encode()))
            .map_err(map_storage_error)?
            .next()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn last(&mut self) -> PairResult<T> {
        let item = self
            .table
            .iter()
            .map_err(map_storage_error)?
            .next_back()
            .transpose()
            .map_err(map_storage_error)?;
        self.current = decode_redb_item::<T>(item)?;
        Ok(self.current.clone())
    }

    fn current(&mut self) -> PairResult<T> {
        Ok(self.current.clone())
    }

    fn walk<'cursor>(
        &'cursor mut self,
        start_key: Option<<T as Table>::Key>,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        let start =
            if let Some(start_key) = start_key { self.seek(start_key) } else { self.first() }
                .transpose();
        Ok(Walker::new(self, start))
    }

    fn walk_range<'cursor>(
        &'cursor mut self,
        range: impl std::ops::RangeBounds<<T as Table>::Key>,
    ) -> Result<RangeWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        let start = match range.start_bound().cloned() {
            Bound::Included(key) => self.seek(key),
            Bound::Excluded(_) => {
                unreachable!("rust doesn't allow for Bound::Excluded in starting bounds")
            }
            Bound::Unbounded => self.first(),
        }
        .transpose();

        Ok(RangeWalker::new(self, start, range.end_bound().cloned()))
    }

    fn walk_back<'cursor>(
        &'cursor mut self,
        start_key: Option<<T as Table>::Key>,
    ) -> Result<ReverseWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        let start =
            if let Some(start_key) = start_key { self.seek(start_key) } else { self.last() }
                .transpose();

        Ok(ReverseWalker::new(self, start))
    }
}

pub struct DupCursorRO<'tx, T: Table> {
    table: MultimapTable<'tx, 'tx, Data, Data>,
    current: Option<KeyValue<T>>,
}

impl<'tx, T: DupSort> DbDupCursorRO<'tx, T> for DupCursorRO<'tx, T> {
    fn next_dup(&mut self) -> PairResult<T> {
        todo!()
    }

    fn next_no_dup(&mut self) -> PairResult<T> {
        todo!()
    }

    fn next_dup_val(&mut self) -> crate::common::ValueOnlyResult<T> {
        todo!()
    }

    fn seek_by_key_subkey(
        &mut self,
        _key: T::Key,
        _subkey: T::SubKey,
    ) -> crate::common::ValueOnlyResult<T> {
        todo!()
    }

    fn walk_dup<'cursor>(
        &'cursor mut self,
        _key: Option<T::Key>,
        _subkey: Option<T::SubKey>,
    ) -> Result<crate::cursor::DupWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<'tx, T: Table> DbCursorRO<'tx, T> for DupCursorRO<'tx, T> {
    fn first(&mut self) -> PairResult<T> {
        todo!()
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        todo!()
    }

    fn seek(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        todo!()
    }

    fn next(&mut self) -> PairResult<T> {
        todo!()
    }

    fn prev(&mut self) -> PairResult<T> {
        todo!()
    }

    fn last(&mut self) -> PairResult<T> {
        todo!()
    }

    fn current(&mut self) -> PairResult<T> {
        todo!()
    }

    fn walk<'cursor>(
        &'cursor mut self,
        _start_key: Option<<T as Table>::Key>,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_range<'cursor>(
        &'cursor mut self,
        _range: impl std::ops::RangeBounds<<T as Table>::Key>,
    ) -> Result<RangeWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_back<'cursor>(
        &'cursor mut self,
        _start_key: Option<<T as Table>::Key>,
    ) -> Result<ReverseWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }
}

pub struct DupCursorRW<'tx, T> {
    table: MultimapTable<'tx, 'tx, Data, Data>,
    _todo: PhantomData<&'tx T>,
}

impl<'tx, T: DupSort> DbDupCursorRO<'tx, T> for DupCursorRW<'tx, T> {
    fn next_dup(&mut self) -> PairResult<T> {
        todo!()
    }

    fn next_no_dup(&mut self) -> PairResult<T> {
        todo!()
    }

    fn next_dup_val(&mut self) -> crate::common::ValueOnlyResult<T> {
        todo!()
    }

    fn seek_by_key_subkey(
        &mut self,
        _key: T::Key,
        _subkey: T::SubKey,
    ) -> crate::common::ValueOnlyResult<T> {
        todo!()
    }

    fn walk_dup<'cursor>(
        &'cursor mut self,
        _key: Option<T::Key>,
        _subkey: Option<T::SubKey>,
    ) -> Result<crate::cursor::DupWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<'tx, T: DupSort> DbDupCursorRW<'tx, T> for DupCursorRW<'tx, T> {
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        todo!()
    }

    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        todo!()
    }
}

impl<'tx, T: Table> DbCursorRO<'tx, T> for DupCursorRW<'tx, T> {
    fn first(&mut self) -> PairResult<T> {
        todo!()
    }

    fn seek_exact(&mut self, _key: <T as Table>::Key) -> PairResult<T> {
        todo!()
    }

    fn seek(&mut self, _key: <T as Table>::Key) -> PairResult<T> {
        todo!()
    }

    fn next(&mut self) -> PairResult<T> {
        todo!()
    }

    fn prev(&mut self) -> PairResult<T> {
        todo!()
    }

    fn last(&mut self) -> PairResult<T> {
        todo!()
    }

    fn current(&mut self) -> PairResult<T> {
        todo!()
    }

    fn walk<'cursor>(
        &'cursor mut self,
        _start_key: Option<<T as Table>::Key>,
    ) -> Result<crate::cursor::Walker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_range<'cursor>(
        &'cursor mut self,
        _range: impl std::ops::RangeBounds<<T as Table>::Key>,
    ) -> Result<crate::cursor::RangeWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_back<'cursor>(
        &'cursor mut self,
        _start_key: Option<<T as Table>::Key>,
    ) -> Result<crate::cursor::ReverseWalker<'cursor, 'tx, T, Self>, DatabaseError>
    where
        Self: Sized,
    {
        todo!()
    }
}
impl<'tx, T: Table> DbCursorRW<'tx, T> for DupCursorRW<'tx, T> {
    fn upsert(
        &mut self,
        key: <T as Table>::Key,
        value: <T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        let key = Data(Bytes::copy_from_slice(key.encode().as_ref()));
        let value = Data(Bytes::copy_from_slice(value.compress().as_ref()));
        self.table.insert(key, value).map_err(map_storage_error)?;
        Ok(())
    }

    fn insert(
        &mut self,
        key: <T as Table>::Key,
        value: <T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        let key = Data(Bytes::copy_from_slice(key.encode().as_ref()));
        let value = Data(Bytes::copy_from_slice(value.compress().as_ref()));

        if self.table.insert(key, value).map_err(map_storage_error)? {
            // todo return err (k/v already exists?)
            todo!()
        }

        Ok(())
    }

    // todo: this optimization doesn't exist in redb
    fn append(
        &mut self,
        key: <T as Table>::Key,
        value: <T as Table>::Value,
    ) -> Result<(), DatabaseError> {
        self.insert(key, value)
    }

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        todo!()
    }
}
