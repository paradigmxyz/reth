//! Mock database WIP
//!
//! Not implemented and put on hold as task of low priority
//! If mockable database is needed you can use mdbx with tempfile.
//!
//! About implementation ideas, DubCursorMut and CursorMut are most troublesome functionality to
//! add. First problem is that written/deleted values are pending until commited, and second is
//! iteration on both written and original values is hard, we could allways copy whole tables from
//! DB to TxMut and transform it again in cursor that support going back and fort in arbitrary order
//! for Cursor. Rusts LinkedList does supports `Cursor` that we could use. BTreeMap does not and to
//! implement it in proper way would require access to not exposed internals. This is maybe worth
//! investigating: https://amanieu.github.io/intrusive-rs/intrusive_collections/rbtree/index.html
use super::{
    Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DbTx, DbTxGAT,
    DbTxMut, DbTxMutGAT, Decode, DupSort, DupWalker, Encode, Error, Table, Walker,
};
use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Structure to store tables and its value for both ordinary and dup tables.
type MockTables = BTreeMap<String, RwLock<BTreeMap<Bytes, BTreeSet<Bytes>>>>;

/// Pending changes used for TxMut.
type PendingChanges = BTreeMap<String, (bool, BTreeMap<Bytes, BTreeMap<Bytes, bool>>)>;

/// Mock database used for testing with inner BTreeMap structure.
///
/// There are few restrictions with comprison with real db:
/// * There is not snapshoting. That mean that you values can be overwriten with some RW
///   transaction.
/// * Cursors are not optimized and will copy whole table from Database and extend it with RW
///   changes.
/// * Values are in Vec<Bytes> to accomodate DupSort
#[derive(Clone)]
pub struct DatabaseMock {
    /// Main data containing tables
    pub tables: Arc<MockTables>,
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
    tables: Arc<MockTables>,
    /// Pending changes for writable trait. Used only for DbTxMut.
    /// bool represent if table was cleared or not. On commit it will replace table with empty
    /// BTreeMap;
    /// BT( TABLE_NAME -> (was_cleared, BT(KEY -> BT(VALUE-> deleted))))
    changes: RwLock<PendingChanges>,
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
        if let Some((_, change_table)) = self.changes.read().get(T::NAME) {
            if let Some(value) = change_table.get(&key) {
                //let value = value.clone();
                return value
                    .iter()
                    .find_map(
                        |(value, is_deleted)| {
                            if !is_deleted {
                                Some(decode_one::<T>(value))
                            } else {
                                None
                            }
                        },
                    )
                    .transpose()
            }
        }
        let table =
            self.tables.get(T::NAME).ok_or_else(|| Error::TableNotExist(String::from(T::NAME)))?;

        table.read().get(&key).and_then(|dup| dup.iter().next().map(decode_one::<T>)).transpose()
    }

    fn commit(self) -> Result<bool, Error> {
        if self.is_mutable {
            for (name, (cleared, table_change)) in self.changes.read().iter() {
                if let Some(table) = self.tables.get(name) {
                    // insert entries
                    let mut table = table.write();
                    if *cleared {
                        table.clear();
                    }
                    for (key, changed_values) in table_change.iter() {
                        #[allow(clippy::mutable_key_type)]
                        let values = table.entry(key.clone()).or_default();
                        for (change_value, is_deleted) in changed_values {
                            if *is_deleted {
                                values.insert(change_value.clone());
                            } else {
                                values.remove(key);
                            }
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
            .1
            .entry(encode_to_bytes(key))
            .or_default()
            .insert(encode_to_bytes(value), false);
        Ok(())
    }

    fn delete<T: Table>(&self, key: T::Key, value: Option<T::Value>) -> Result<bool, Error> {
        // check if table exist
        if !self.tables.contains_key(T::NAME) {
            return Err(Error::TableNotExist(String::from(T::NAME)))
        }
        let mut changes = self.changes.write();

        #[allow(clippy::mutable_key_type)]
        let values = &mut changes.entry(String::from(T::NAME)).or_default().1;
        if let Some(_value) = value {
            //values.entry(value).map(|t|T)
            // remove from values only entry that matches value
            // This is significant for duplicate indexes table (`DupSort`)
        } else {
            // remove first
        }
        let prev_value = values.remove(&encode_to_bytes(key));
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
        self.changes.write().entry(String::from(T::NAME)).or_default().0 = true;
        Ok(())
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
