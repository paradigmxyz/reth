//! Cursor wrapper for libmdbx-sys.

use super::{cursor_cache::CursorCache, utils::*};
use crate::{
    metrics::{DatabaseEnvMetrics, Operation},
    DatabaseError,
};
use reth_db_api::{
    common::{PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    table::{Compress, Decode, Decompress, DupSort, Encode, IntoVec, Table},
};
use reth_libmdbx::{ffi, Error as MDBXError, TransactionKind, WriteFlags, RO, RW};
use reth_storage_errors::db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation};
use std::{borrow::Cow, collections::Bound, marker::PhantomData, ops::RangeBounds, sync::Arc};

/// Read only Cursor.
pub type CursorRO<'tx, T> = Cursor<'tx, RO, T>;
/// Read write cursor.
pub type CursorRW<'tx, T> = Cursor<'tx, RW, T>;

/// Cursor wrapper to access KV items.
///
/// When dropped, the cursor is returned to the transaction's cursor cache for reuse,
/// rather than being closed. This reduces the overhead of cursor creation in
/// cursor-heavy workloads.
#[derive(Debug)]
pub struct Cursor<'tx, K: TransactionKind, T: Table> {
    /// Inner `libmdbx` cursor. Wrapped in Option so we can take it in Drop.
    pub(crate) inner: Option<reth_libmdbx::Cursor<'tx, K>>,
    /// Cache buffer that receives compressed values.
    buf: Vec<u8>,
    /// Reference to metric handles in the DB environment. If `None`, metrics are not recorded.
    metrics: Option<Arc<DatabaseEnvMetrics>>,
    /// Reference to the cursor cache where this cursor should be returned on drop.
    /// If `None`, the cursor is closed normally on drop.
    cache: Option<&'tx CursorCache>,
    /// Phantom data to enforce encoding/decoding.
    _dbi: PhantomData<T>,
}

#[expect(clippy::missing_const_for_fn)]
impl<'tx, K: TransactionKind, T: Table> Cursor<'tx, K, T> {
    pub(crate) fn new_with_metrics(
        inner: reth_libmdbx::Cursor<'tx, K>,
        metrics: Option<Arc<DatabaseEnvMetrics>>,
    ) -> Self {
        Self { inner: Some(inner), buf: Vec::new(), metrics, cache: None, _dbi: PhantomData }
    }

    pub(crate) fn new_with_cache(
        inner: reth_libmdbx::Cursor<'tx, K>,
        metrics: Option<Arc<DatabaseEnvMetrics>>,
        cache: &'tx CursorCache,
    ) -> Self {
        Self { inner: Some(inner), buf: Vec::new(), metrics, cache: Some(cache), _dbi: PhantomData }
    }

    /// Returns a mutable reference to the inner cursor.
    #[inline]
    fn inner_mut(&mut self) -> &mut reth_libmdbx::Cursor<'tx, K> {
        self.inner.as_mut().expect("cursor already taken")
    }

    /// If `self.metrics` is `Some(...)`, record a metric with the provided operation and value
    /// size.
    ///
    /// Otherwise, just execute the closure.
    fn execute_with_operation_metric<R>(
        &mut self,
        operation: Operation,
        value_size: Option<usize>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if let Some(metrics) = self.metrics.clone() {
            metrics.record_operation(T::NAME, operation, value_size, || f(self))
        } else {
            f(self)
        }
    }
}

impl<K: TransactionKind, T: Table> Drop for Cursor<'_, K, T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() &&
            let Some(cache) = self.cache
        {
            // Return cursor to cache for reuse
            let raw = inner.into_raw();
            let old = cache.put(T::INDEX, raw);
            // If there was already a cursor in the slot (shouldn't happen), close it
            if let Some(old_cursor) = old {
                unsafe {
                    ffi::mdbx_cursor_close(old_cursor);
                }
            }
        }
        // If no cache or no inner, the cursor drops normally and closes itself
    }
}

/// Decodes a `(key, value)` pair from the database.
#[expect(clippy::type_complexity)]
pub fn decode<T>(
    res: Result<Option<(Cow<'_, [u8]>, Cow<'_, [u8]>)>, impl Into<DatabaseErrorInfo>>,
) -> PairResult<T>
where
    T: Table,
    T::Key: Decode,
    T::Value: Decompress,
{
    res.map_err(|e| DatabaseError::Read(e.into()))?.map(decoder::<T>).transpose()
}

/// Some types don't support compression (eg. B256), and we don't want to be copying them to the
/// allocated buffer when we can just use their reference.
macro_rules! compress_to_buf_or_ref {
    ($self:expr, $value:expr) => {
        if let Some(value) = $value.uncompressable_ref() {
            Some(value)
        } else {
            $self.buf.clear();
            $value.compress_to_buf(&mut $self.buf);
            None
        }
    };
}

impl<K: TransactionKind, T: Table> DbCursorRO<T> for Cursor<'_, K, T> {
    fn first(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().first())
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        decode::<T>(self.inner_mut().set_key(key.encode().as_ref()))
    }

    fn seek(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        decode::<T>(self.inner_mut().set_range(key.encode().as_ref()))
    }

    fn next(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().next())
    }

    fn prev(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().prev())
    }

    fn last(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().last())
    }

    fn current(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().get_current())
    }

    fn walk(&mut self, start_key: Option<T::Key>) -> Result<Walker<'_, T, Self>, DatabaseError> {
        let start = if let Some(start_key) = start_key {
            decode::<T>(self.inner_mut().set_range(start_key.encode().as_ref())).transpose()
        } else {
            self.first().transpose()
        };

        Ok(Walker::new(self, start))
    }

    fn walk_range(
        &mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'_, T, Self>, DatabaseError> {
        let start = match range.start_bound().cloned() {
            Bound::Included(key) => self.inner_mut().set_range(key.encode().as_ref()),
            Bound::Excluded(_key) => {
                unreachable!("Rust doesn't allow for Bound::Excluded in starting bounds");
            }
            Bound::Unbounded => self.inner_mut().first(),
        };
        let start = decode::<T>(start).transpose();
        Ok(RangeWalker::new(self, start, range.end_bound().cloned()))
    }

    fn walk_back(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError> {
        let start = if let Some(start_key) = start_key {
            decode::<T>(self.inner_mut().set_range(start_key.encode().as_ref()))
        } else {
            self.last()
        }
        .transpose();

        Ok(ReverseWalker::new(self, start))
    }
}

impl<K: TransactionKind, T: DupSort> DbDupCursorRO<T> for Cursor<'_, K, T> {
    /// Returns the previous `(key, value)` pair of a DUPSORT table.
    fn prev_dup(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().prev_dup())
    }

    /// Returns the next `(key, value)` pair of a DUPSORT table.
    fn next_dup(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().next_dup())
    }

    /// Returns the last `value` of the current duplicate `key`.
    fn last_dup(&mut self) -> ValueOnlyResult<T> {
        self.inner_mut()
            .last_dup()
            .map_err(|e| DatabaseError::Read(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T> {
        decode::<T>(self.inner_mut().next_nodup())
    }

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        self.inner_mut()
            .next_dup()
            .map_err(|e| DatabaseError::Read(e.into()))?
            .map(decode_value::<T>)
            .transpose()
    }

    fn seek_by_key_subkey(
        &mut self,
        key: <T as Table>::Key,
        subkey: <T as DupSort>::SubKey,
    ) -> ValueOnlyResult<T> {
        self.inner_mut()
            .get_both_range(key.encode().as_ref(), subkey.encode().as_ref())
            .map_err(|e| DatabaseError::Read(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }

    /// Depending on its arguments, returns an iterator starting at:
    /// - Some(key), Some(subkey): a `key` item whose data is >= than `subkey`
    /// - Some(key), None: first item of a specified `key`
    /// - None, Some(subkey): like first case, but in the first key
    /// - None, None: first item in the table of a DUPSORT table.
    fn walk_dup(
        &mut self,
        key: Option<T::Key>,
        subkey: Option<T::SubKey>,
    ) -> Result<DupWalker<'_, T, Self>, DatabaseError> {
        let start = match (key, subkey) {
            (Some(key), Some(subkey)) => {
                let encoded_key = key.encode();
                self.inner_mut()
                    .get_both_range(encoded_key.as_ref(), subkey.encode().as_ref())
                    .map_err(|e| DatabaseError::Read(e.into()))?
                    .map(|val| decoder::<T>((Cow::Borrowed(encoded_key.as_ref()), val)))
            }
            (Some(key), None) => {
                let encoded_key = key.encode();
                self.inner_mut()
                    .set(encoded_key.as_ref())
                    .map_err(|e| DatabaseError::Read(e.into()))?
                    .map(|val| decoder::<T>((Cow::Borrowed(encoded_key.as_ref()), val)))
            }
            (None, Some(subkey)) => {
                if let Some((key, _)) = self.first()? {
                    let encoded_key = key.encode();
                    self.inner_mut()
                        .get_both_range(encoded_key.as_ref(), subkey.encode().as_ref())
                        .map_err(|e| DatabaseError::Read(e.into()))?
                        .map(|val| decoder::<T>((Cow::Borrowed(encoded_key.as_ref()), val)))
                } else {
                    Some(Err(DatabaseError::Read(MDBXError::NotFound.into())))
                }
            }
            (None, None) => self.first().transpose(),
        };

        Ok(DupWalker::<'_, T, Self> { cursor: self, start })
    }
}

impl<T: Table> DbCursorRW<T> for Cursor<'_, RW, T> {
    /// Database operation that will update an existing row if a specified value already
    /// exists in a table, and insert a new row if the specified value doesn't already exist
    ///
    /// For a DUPSORT table, `upsert` will not actually update-or-insert. If the key already exists,
    /// it will append the value to the subkey, even if the subkeys are the same. So if you want
    /// to properly upsert, you'll need to `seek_exact` & `delete_current` if the key+subkey was
    /// found, before calling `upsert`.
    fn upsert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorUpsert,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                let value_bytes: &[u8] = match value {
                    Some(v) => v,
                    None => &this.buf,
                };
                let inner = this.inner.as_mut().expect("cursor already taken");
                inner.put(key.as_ref(), value_bytes, WriteFlags::UPSERT).map_err(|e| {
                    DatabaseWriteError {
                        info: e.into(),
                        operation: DatabaseWriteOperation::CursorUpsert,
                        table_name: T::NAME,
                        key: key.into_vec(),
                    }
                    .into()
                })
            },
        )
    }

    fn insert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorInsert,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                let value_bytes: &[u8] = match value {
                    Some(v) => v,
                    None => &this.buf,
                };
                let inner = this.inner.as_mut().expect("cursor already taken");
                inner.put(key.as_ref(), value_bytes, WriteFlags::NO_OVERWRITE).map_err(|e| {
                    DatabaseWriteError {
                        info: e.into(),
                        operation: DatabaseWriteOperation::CursorInsert,
                        table_name: T::NAME,
                        key: key.into_vec(),
                    }
                    .into()
                })
            },
        )
    }

    /// Appends the data to the end of the table. Consequently, the append operation
    /// will fail if the inserted key is less than the last table key
    fn append(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorAppend,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                let value_bytes: &[u8] = match value {
                    Some(v) => v,
                    None => &this.buf,
                };
                let inner = this.inner.as_mut().expect("cursor already taken");
                inner.put(key.as_ref(), value_bytes, WriteFlags::APPEND).map_err(|e| {
                    DatabaseWriteError {
                        info: e.into(),
                        operation: DatabaseWriteOperation::CursorAppend,
                        table_name: T::NAME,
                        key: key.into_vec(),
                    }
                    .into()
                })
            },
        )
    }

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        self.execute_with_operation_metric(Operation::CursorDeleteCurrent, None, |this| {
            this.inner_mut().del(WriteFlags::CURRENT).map_err(|e| DatabaseError::Delete(e.into()))
        })
    }
}

impl<T: DupSort> DbDupCursorRW<T> for Cursor<'_, RW, T> {
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        self.execute_with_operation_metric(Operation::CursorDeleteCurrentDuplicates, None, |this| {
            this.inner_mut()
                .del(WriteFlags::NO_DUP_DATA)
                .map_err(|e| DatabaseError::Delete(e.into()))
        })
    }

    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorAppendDup,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                let value_bytes: &[u8] = match value {
                    Some(v) => v,
                    None => &this.buf,
                };
                let inner = this.inner.as_mut().expect("cursor already taken");
                inner.put(key.as_ref(), value_bytes, WriteFlags::APPEND_DUP).map_err(|e| {
                    DatabaseWriteError {
                        info: e.into(),
                        operation: DatabaseWriteOperation::CursorAppendDup,
                        table_name: T::NAME,
                        key: key.into_vec(),
                    }
                    .into()
                })
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        mdbx::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind},
        tables::StorageChangeSets,
        Database,
    };
    use alloy_primitives::{address, Address, B256, U256};
    use reth_db_api::{
        cursor::{DbCursorRO, DbDupCursorRW},
        models::{BlockNumberAddress, ClientVersion},
        table::TableImporter,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::StorageEntry;
    use tempfile::TempDir;

    fn create_test_db() -> DatabaseEnv {
        let path = TempDir::new().unwrap();
        let mut db = DatabaseEnv::open(
            path.path(),
            DatabaseEnvKind::RW,
            DatabaseArguments::new(ClientVersion::default()),
        )
        .unwrap();
        db.create_tables().unwrap();
        db
    }

    #[test]
    fn test_import_table_with_range_works_on_dupsort() {
        let addr1 = address!("0000000000000000000000000000000000000001");
        let addr2 = address!("0000000000000000000000000000000000000002");
        let addr3 = address!("0000000000000000000000000000000000000003");
        let source_db = create_test_db();
        let target_db = create_test_db();
        let test_data = vec![
            (
                BlockNumberAddress((100, addr1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(100) },
            ),
            (
                BlockNumberAddress((100, addr1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::from(200) },
            ),
            (
                BlockNumberAddress((100, addr1)),
                StorageEntry { key: B256::with_last_byte(3), value: U256::from(300) },
            ),
            (
                BlockNumberAddress((101, addr1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(400) },
            ),
            (
                BlockNumberAddress((101, addr2)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(500) },
            ),
            (
                BlockNumberAddress((101, addr2)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::from(600) },
            ),
            (
                BlockNumberAddress((102, addr3)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(700) },
            ),
        ];

        // setup data
        let tx = source_db.tx_mut().unwrap();
        {
            let mut cursor = tx.cursor_dup_write::<StorageChangeSets>().unwrap();
            for (key, value) in &test_data {
                cursor.append_dup(*key, *value).unwrap();
            }
        }
        tx.commit().unwrap();

        // import data from source db to target
        let source_tx = source_db.tx().unwrap();
        let target_tx = target_db.tx_mut().unwrap();

        target_tx
            .import_table_with_range::<StorageChangeSets, _>(
                &source_tx,
                Some(BlockNumberAddress((100, Address::ZERO))),
                BlockNumberAddress((102, Address::repeat_byte(0xff))),
            )
            .unwrap();
        target_tx.commit().unwrap();

        // fetch all data from target db
        let verify_tx = target_db.tx().unwrap();
        let mut cursor = verify_tx.cursor_dup_read::<StorageChangeSets>().unwrap();
        let copied: Vec<_> = cursor.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();

        // verify each entry matches the test data
        assert_eq!(copied.len(), test_data.len(), "Should copy all entries including duplicates");
        for ((copied_key, copied_value), (expected_key, expected_value)) in
            copied.iter().zip(test_data.iter())
        {
            assert_eq!(copied_key, expected_key);
            assert_eq!(copied_value, expected_value);
        }
    }
}
