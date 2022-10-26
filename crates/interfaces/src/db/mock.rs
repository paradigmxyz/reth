//! Mock database
use super::{
    Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DbTx, DbTxGAT,
    DbTxMut, DbTxMutGAT, Decode, DupSort, DupWalker, Encode, Error, Table, Walker,
};
use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Mock database used for testing with inner BTreeMap structure
#[derive(Clone)]
pub struct DatabaseMock {
    /// Main data containing tables
    pub tables: Arc<BTreeMap<String, RwLock<BTreeMap<Bytes, Bytes>>>>,
    /// It can emulate RO database if set to false. For RW database it is needed
    /// to flag when mutable transaction is created as only one of those transaction
    /// can alive.
    pub has_mutability: Arc<AtomicBool>,
}

impl DatabaseMock {
    /// Create mock database with table names.
    pub fn create(tables: &[&str]) -> Self {
        let mut db_tables = BTreeMap::new();
        for &table in tables {
            db_tables.insert(String::from(table), RwLock::new(BTreeMap::new()));
        }
        Self { tables: Arc::new(db_tables), has_mutability: Arc::new(AtomicBool::new(true)) }
    }
}

impl Database for DatabaseMock {
    fn tx(&self) -> Result<<Self as DatabaseGAT<'_>>::TX, Error> {
        let tables = self.tables.clone();
        Ok(TxMock { tables, changes: Default::default(), is_mutable: true })
    }

    fn tx_mut(&self) -> Result<<Self as DatabaseGAT<'_>>::TXMut, Error> {
        let tables = self.tables.clone();
        if self.has_mutability.fetch_and(false, Ordering::SeqCst) {
            let tx = TxMock { tables, changes: Default::default(), is_mutable: true };
            Ok(tx)
        } else {
            Err(Error::PermissionDenied)
        }
    }
}

impl<'a> DatabaseGAT<'a> for DatabaseMock {
    type TX = TxMock;

    type TXMut = TxMock;
}

/// Mock transaction
pub struct TxMock {
    /// Table representation
    tables: Arc<BTreeMap<String, RwLock<BTreeMap<Bytes, Bytes>>>>,
    /// Pending changes for writable trait. Used only for DbTxMut.
    changes: RwLock<BTreeMap<String, BTreeMap<Bytes, Option<Bytes>>>>,
    /// Is mutable transaction.
    is_mutable: bool,
}

impl<'a> DbTxGAT<'a> for TxMock {
    type Cursor<T: Table> = CursorMock;
    type DupCursor<T: DupSort> = CursorMock;
}

impl<'a> DbTxMutGAT<'a> for TxMock {
    type CursorMut<T: Table> = CursorMock;
    type DupCursorMut<T: DupSort> = CursorMock;
}

// /// Helper function to decode a value. It can be a key or subkey.
pub(crate) fn decode_one<T: Table>(value: &Bytes) -> Result<T::Value, Error> {
    Decode::decode(value.clone())
}

pub(crate) fn encode_to_bytes<T: Encode>(key: T) -> Bytes {
    Bytes::from(BytesMut::from(key.encode().as_ref()))
}

impl<'a> DbTx<'a> for TxMock {
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, Error> {
        let key = Bytes::from(BytesMut::from(key.encode().as_ref()));
        if let Some(change_table) = self.changes.read().get(T::NAME) {
            if let Some(value) = change_table.get(&key) {
                let value = value.clone();
                return value.map(|t| decode_one::<T>(&t)).transpose()
            }
        }
        let table = self.tables.get(T::NAME).ok_or(Error::TableNotExist(String::from(T::NAME)))?;

        table.read().get(&key).map(decode_one::<T>).transpose()
    }

    fn commit(self) -> Result<bool, Error> {
        if self.is_mutable {
            for (name, table_change) in self.changes.read().iter() {
                if let Some(table) = self.tables.get(name) {
                    // insert entries
                    let mut table = table.write();
                    for (key, value) in table_change.into_iter() {
                        if let Some(value) = value {
                            table.insert(key.clone(), value.clone());
                        } else {
                            table.remove(key);
                        }
                    }
                }
            }
            Ok(true)
        } else {
            Ok(true)
        }
    }

    fn cursor<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, Error> {
        todo!()
    }

    fn cursor_dup<T: super::DupSort>(&self) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, Error> {
        todo!()
    }
}

impl<'a> DbTxMut<'a> for TxMock {
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), Error> {
        // check if table exist
        if !self.tables.contains_key(T::NAME) {
            return Err(Error::TableNotExist(String::from(T::NAME)))
        }
        self.changes
            .write()
            .entry(String::from(T::NAME))
            .or_default()
            .insert(encode_to_bytes(key), Some(encode_to_bytes(value)));
        Ok(())
    }

    fn delete<T: Table>(&self, key: T::Key, _value: Option<T::Value>) -> Result<bool, Error> {
        // check if table exist
        if !self.tables.contains_key(T::NAME) {
            return Err(Error::TableNotExist(String::from(T::NAME)))
        }
        let prev_value = self.changes
            .write()
            .entry(String::from(T::NAME))
            .or_default()
            .remove(&encode_to_bytes(key));
        // TODO do check for _value
        Ok(prev_value.is_some())
    }

    fn cursor_mut<T: Table>(&self) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, Error> {
        // check if table exist
        if !self.tables.contains_key(T::NAME) {
            return Err(Error::TableNotExist(String::from(T::NAME)))
        }
        todo!()
    }

    fn cursor_dup_mut<T: super::DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, Error> {
        // check if table exist
        if !self.tables.contains_key(T::NAME) {
            return Err(Error::TableNotExist(String::from(T::NAME)))
        }
        todo!()
    }

    fn clear<T: Table>(&self) -> Result<(), Error> {
        // check if table exist TODO
        if !self.tables.contains_key(T::NAME) {
            return Err(Error::TableNotExist(String::from(T::NAME)))
        }
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

    fn seek(&mut self, _key: T::SeekKey) -> super::PairResult<T> {
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
