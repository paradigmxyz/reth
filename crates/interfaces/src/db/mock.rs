//! Mock database
use std::collections::BTreeMap;

use super::{
    Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DbTx, DbTxGAT,
    DbTxMut, DbTxMutGAT, DupSort, DupWalker, Error, Table, Walker,
};

/// Mock database used for testing with inner BTreeMap structure
/// TODO
#[derive(Clone, Default)]
pub struct DatabaseMock {
    /// Main data. TODO (Make it table aware)
    pub data: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Database for DatabaseMock {
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, super::Error> {
        Ok(TxMock::default())
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, super::Error> {
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
    fn get<T: super::Table>(&self, _key: T::Key) -> Result<Option<T::Value>, super::Error> {
        todo!()
    }

    fn commit(self) -> Result<bool, super::Error> {
        todo!()
    }

    fn cursor<T: super::Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, super::Error> {
        todo!()
    }

    fn cursor_dup<T: super::DupSort>(
        &self,
    ) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, super::Error> {
        todo!()
    }
}

impl<'a> DbTxMut<'a> for TxMock {
    fn put<T: super::Table>(&self, _key: T::Key, _value: T::Value) -> Result<(), super::Error> {
        todo!()
    }

    fn delete<T: super::Table>(
        &self,
        _key: T::Key,
        _value: Option<T::Value>,
    ) -> Result<bool, super::Error> {
        todo!()
    }

    fn cursor_mut<T: super::Table>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, super::Error> {
        todo!()
    }

    fn cursor_dup_mut<T: super::DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, super::Error> {
        todo!()
    }

    fn clear<T: Table>(&self) -> Result<(), super::Error> {
        todo!()
    }
}

/// CUrsor that iterates over table
pub struct CursorMock {
    _cursor: u32,
}

impl<'tx, T: Table> DbCursorRO<'tx, T> for CursorMock {
    fn first(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn seek_exact(&mut self, _key: T::Key) -> super::PairResult<T> {
        todo!()
    }

    fn next(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn prev(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn last(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn current(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn walk<'cursor>(
        &'cursor mut self,
        _start_key: T::Key,
    ) -> Result<Walker<'cursor, 'tx, T, Self>, Error>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl<'tx, T: DupSort> DbDupCursorRO<'tx, T> for CursorMock {
    fn next_dup(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn seek(&mut self, _key: T::SubKey) -> super::PairResult<T> {
        todo!()
    }

    fn next_no_dup(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn next_dup_val(&mut self) -> super::ValueOnlyResult<T> {
        todo!()
    }

    fn walk_dup<'cursor>(
        &'cursor mut self,
        _key: <T>::Key,
        _subkey: <T as DupSort>::SubKey,
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
    ) -> Result<(), super::Error> {
        todo!()
    }

    fn insert(
        &mut self,
        _key: <T as Table>::Key,
        _value: <T as Table>::Value,
    ) -> Result<(), super::Error> {
        todo!()
    }

    fn append(
        &mut self,
        _key: <T as Table>::Key,
        _value: <T as Table>::Value,
    ) -> Result<(), super::Error> {
        todo!()
    }

    fn delete_current(&mut self) -> Result<(), super::Error> {
        todo!()
    }
}

impl<'tx, T: DupSort> DbDupCursorRW<'tx, T> for CursorMock {
    fn delete_current_duplicates(&mut self) -> Result<(), super::Error> {
        todo!()
    }

    fn append_dup(&mut self, _key: <T>::Key, _value: <T>::Value) -> Result<(), super::Error> {
        todo!()
    }
}
