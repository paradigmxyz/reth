//! Mock database
//! 
use std::collections::BTreeMap;

use super::{Database, DbCursorRO, DbTx, DbTxMut, Table, DupSort, DbDupCursorRO, DbCursorRW, DbDupCursorRW};

/// Mock database used for testing with inner BTreeMap structure
pub struct DatabaseMock {
    /// Main data. TODO (Make it table aware)
    pub data: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Default for DatabaseMock {
    fn default() -> Self {
        Self { data: BTreeMap::new() }
    }
}

impl Database for DatabaseMock {
    type TX<'a> = TxMock;

    type TXMut<'a> = TxMock;

    fn tx<'a>(&'a self) -> Result<Self::TX<'a>, super::Error> {
        Ok(TxMock::default())
    }

    fn tx_mut<'a>(&'a self) -> Result<Self::TXMut<'a>, super::Error> {
        Ok(TxMock::default())
    }
}

/// Mock read only tx
pub struct TxMock {
    /// Table representation
    table: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Default for TxMock {
    fn default() -> Self {
        Self {
            table: BTreeMap::new(),
        }
    }
}

impl<'a> DbTx<'a> for TxMock {
    type Cursor<T: super::Table> = CursorMock;

    type DupCursor<T: super::DupSort> = CursorMock;

    fn get<T: super::Table>(&self, key: T::Key) -> Result<Option<T::Value>, super::Error> {
        todo!()
    }

    fn commit(self) -> Result<bool, super::Error> {
        todo!()
    }

    fn cursor<T: super::Table>(&self) -> Result<Self::Cursor<T>, super::Error> {
        todo!()
    }

    fn cursor_dup<T: super::DupSort>(&self) -> Result<Self::DupCursor<T>, super::Error> {
        todo!()
    }
}

impl<'a> DbTxMut<'a> for TxMock {
    type CursorMut<T: super::Table> = CursorMock;

    type DupCursorMut<T: super::DupSort> = CursorMock;

    fn put<T: super::Table>(&self, key: T::Key, value: T::Value) -> Result<(), super::Error> {
        todo!()
    }

    fn delete<T: super::Table>(
        &self,
        key: T::Key,
        value: Option<T::Value>,
    ) -> Result<bool, super::Error> {
        todo!()
    }

    fn cursor_mut<T: super::Table>(&self) -> Result<Self::CursorMut<T>, super::Error> {
        todo!()
    }

    fn cursor_dup_mut<T: super::DupSort>(&self) -> Result<Self::DupCursorMut<T>, super::Error> {
        todo!()
    }
}

/// CUrsor that iterates over table
pub struct CursorMock {
    cursor: u32,
}

impl<'tx, T: Table> DbCursorRO<'tx, T> for CursorMock {
    fn first(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn seek(&mut self, key: T::SeekKey) -> super::PairResult<T> {
        todo!()
    }

    fn seek_exact(&mut self, key: T::Key) -> super::PairResult<T> {
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

    fn walk(&'tx mut self, start_key: T::Key) -> Result<super::Walker<'tx, T>, super::Error> {
        todo!()
    }
}


impl<'tx, T: DupSort> DbDupCursorRO<'tx, T> for CursorMock {
    fn next_dup(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn next_no_dup(&mut self) -> super::PairResult<T> {
        todo!()
    }

    fn next_dup_val(&mut self) -> super::ValueOnlyResult<T> {
        todo!()
    }

    fn walk_dup(&'tx mut self, key: <T>::Key, subkey: <T as DupSort>::SubKey) -> Result<super::DupWalker<'tx, T>, super::Error> {
        todo!()
    }
}


impl<'tx, T: Table> DbCursorRW<'tx, T> for CursorMock {
    fn put(
        &mut self,
        k: <T as Table>::Key,
        v: <T as Table>::Value, /* , f: Option<WriteFlags> */
    ) -> Result<(), super::Error> {
        todo!()
    }
}

impl<'tx, T: DupSort> DbDupCursorRW<'tx, T> for CursorMock {
    fn put(
        &mut self,
        k: <T>::Key,
        v: <T>::Value, /* , f: Option<WriteFlags> */
    ) -> Result<(), super::Error> {
        todo!()
    }
}