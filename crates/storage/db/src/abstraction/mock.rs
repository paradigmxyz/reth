//! Mock database
use std::{collections::BTreeMap, ops::RangeBounds};

use crate::{
    common::{PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    database::{Database, DatabaseGAT},
    table::{DupSort, Table, TableImporter},
    transaction::{DbTx, DbTxGAT, DbTxMut, DbTxMutGAT},
    Error,
};

/// Mock database used for testing with inner BTreeMap structure
/// TODO
#[derive(Clone, Default)]
pub struct DatabaseMock {
    /// Main data. TODO (Make it table aware)
    pub data: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Database for DatabaseMock {
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, Error> {
        Ok(TxMock::default())
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, Error> {
        Ok(TxMock::default())
    }
}

impl<'a> DatabaseGAT<'a> for DatabaseMock {
    type TX = TxMock;

    type TXMut = TxMock;
}

/// Mock read only tx
#[derive(Clone, Default)]
pub struct TxMock {
    /// Table representation
    _table: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl<'a> DbTxGAT<'a> for TxMock {
    type Cursor<T: Table> = CursorMock;
    type DupCursor<T: DupSort> = CursorMock;
}

impl<'a> DbTxMutGAT<'a> for TxMock {
    type CursorMut<T: Table> = CursorMock;
    type DupCursorMut<T: DupSort> = CursorMock;
}

impl<'a> DbTx<'a> for TxMock {
    fn get<T: Table>(&self, _key: T::Key) -> Result<Option<T::Value>, Error> {
        todo!()
    }

    fn commit(self) -> Result<bool, Error> {
        todo!()
    }

    fn drop(self) {
        todo!()
    }

    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, Error> {
        todo!()
    }

    fn cursor_dup_read<T: DupSort>(&self) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, Error> {
        todo!()
    }
}

impl<'a> DbTxMut<'a> for TxMock {
    fn put<T: Table>(&self, _key: T::Key, _value: T::Value) -> Result<(), Error> {
        todo!()
    }

    fn delete<T: Table>(&self, _key: T::Key, _value: Option<T::Value>) -> Result<bool, Error> {
        todo!()
    }

    fn cursor_write<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error> {
        todo!()
    }

    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error> {
        todo!()
    }

    fn clear<T: Table>(&self) -> Result<(), Error> {
        todo!()
    }
}

impl<'a> TableImporter<'a> for TxMock {}

/// Cursor that iterates over table
pub struct CursorMock {
    _cursor: u32,
}

impl<'tx, T: Table> DbCursorRO<'tx, T> for CursorMock {
    fn first(&mut self) -> PairResult<T> {
        todo!()
    }

    fn seek_exact(&mut self, _key: T::Key) -> PairResult<T> {
        todo!()
    }

    fn seek(&mut self, _key: T::Key) -> PairResult<T> {
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
        _start_key: Option<T::Key>,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_range<'cursor>(
        &'cursor mut self,
        _range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized,
    {
        todo!()
    }

    fn walk_back<'cursor>(
        &'cursor mut self,
        _start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<'tx, T: DupSort> DbDupCursorRO<'tx, T> for CursorMock {
    fn next_dup(&mut self) -> PairResult<T> {
        todo!()
    }

    fn next_no_dup(&mut self) -> PairResult<T> {
        todo!()
    }

    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        todo!()
    }

    fn seek_by_key_subkey(
        &mut self,
        _key: <T as Table>::Key,
        _subkey: <T as DupSort>::SubKey,
    ) -> ValueOnlyResult<T> {
        todo!()
    }

    fn walk_dup<'cursor>(
        &'cursor mut self,
        _key: Option<<T>::Key>,
        _subkey: Option<<T as DupSort>::SubKey>,
    ) -> Result<DupWalker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<'tx, T: Table> DbCursorRW<'tx, T> for CursorMock {
    fn upsert(
        &mut self,
        _key: <T as Table>::Key,
        _value: <T as Table>::Value,
    ) -> Result<(), Error> {
        todo!()
    }

    fn insert(
        &mut self,
        _key: <T as Table>::Key,
        _value: <T as Table>::Value,
    ) -> Result<(), Error> {
        todo!()
    }

    fn append(
        &mut self,
        _key: <T as Table>::Key,
        _value: <T as Table>::Value,
    ) -> Result<(), Error> {
        todo!()
    }

    fn delete_current(&mut self) -> Result<(), Error> {
        todo!()
    }
}

impl<'tx, T: DupSort> DbDupCursorRW<'tx, T> for CursorMock {
    fn delete_current_duplicates(&mut self) -> Result<(), Error> {
        todo!()
    }

    fn append_dup(&mut self, _key: <T>::Key, _value: <T>::Value) -> Result<(), Error> {
        todo!()
    }
}
