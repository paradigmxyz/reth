//! Cursor wrapper for libmdbx-sys.

use super::utils::*;
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
use reth_libmdbx::{Error as MDBXError, TransactionKind, WriteFlags, RO, RW};
use reth_storage_errors::db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation};
use std::{borrow::Cow, collections::Bound, marker::PhantomData, ops::RangeBounds, sync::Arc};

/// Read only Cursor.
pub type CursorRO<T> = Cursor<RO, T>;
/// Read write cursor.
pub type CursorRW<T> = Cursor<RW, T>;

/// Cursor wrapper to access KV items.
#[derive(Debug)]
pub struct Cursor<K: TransactionKind, T: Table> {
    /// Inner `libmdbx` cursor.
    pub(crate) inner: reth_libmdbx::Cursor<K>,
    /// Cache buffer that receives compressed values.
    buf: Vec<u8>,
    /// Reference to metric handles in the DB environment. If `None`, metrics are not recorded.
    metrics: Option<Arc<DatabaseEnvMetrics>>,
    /// Phantom data to enforce encoding/decoding.
    _dbi: PhantomData<T>,
}

impl<K: TransactionKind, T: Table> Cursor<K, T> {
    pub(crate) const fn new_with_metrics(
        inner: reth_libmdbx::Cursor<K>,
        metrics: Option<Arc<DatabaseEnvMetrics>>,
    ) -> Self {
        Self { inner, buf: Vec::new(), metrics, _dbi: PhantomData }
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





impl<K: TransactionKind, T: Table> DbCursorRO<T> for Cursor<K, T> {
    fn first(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.first())
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        decode::<T>(self.inner.set_key(key.encode().as_ref()))
    }

    fn seek(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        decode::<T>(self.inner.set_range(key.encode().as_ref()))
    }

    fn next(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.next())
    }

    fn prev(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.prev())
    }

    fn last(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.last())
    }

    fn current(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.get_current())
    }

    fn walk(&mut self, start_key: Option<T::Key>) -> Result<Walker<'_, T, Self>, DatabaseError> {
        let start = if let Some(start_key) = start_key {
            decode::<T>(self.inner.set_range(start_key.encode().as_ref())).transpose()
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
            Bound::Included(key) => self.inner.set_range(key.encode().as_ref()),
            Bound::Excluded(_key) => {
                unreachable!("Rust doesn't allow for Bound::Excluded in starting bounds");
            }
            Bound::Unbounded => self.inner.first(),
        };
        let start = decode::<T>(start).transpose();
        Ok(RangeWalker::new(self, start, range.end_bound().cloned()))
    }

    fn walk_back(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError> {
        let start = if let Some(start_key) = start_key {
            decode::<T>(self.inner.set_range(start_key.encode().as_ref()))
        } else {
            self.last()
        }
        .transpose();

        Ok(ReverseWalker::new(self, start))
    }
}

impl<K: TransactionKind, T: DupSort> DbDupCursorRO<T> for Cursor<K, T> {
    /// Returns the previous `(key, value)` pair of a DUPSORT table.
    fn prev_dup(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.prev_dup())
    }

    /// Returns the next `(key, value)` pair of a DUPSORT table.
    fn next_dup(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.next_dup())
    }

    /// Returns the last `value` of the current duplicate `key`.
    fn last_dup(&mut self) -> ValueOnlyResult<T> {
        self.inner
            .last_dup()
            .map_err(|e| DatabaseError::Read(e.into()))?
            .map(decode_one::<T>)
            .transpose()
    }

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T> {
        decode::<T>(self.inner.next_nodup())
    }

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        self.inner
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
        self.inner
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
                self.inner
                    .get_both_range(encoded_key.as_ref(), subkey.encode().as_ref())
                    .map_err(|e| DatabaseError::Read(e.into()))?
                    .map(|val| decoder::<T>((Cow::Borrowed(encoded_key.as_ref()), val)))
            }
            (Some(key), None) => {
                let encoded_key = key.encode();
                self.inner
                    .set(encoded_key.as_ref())
                    .map_err(|e| DatabaseError::Read(e.into()))?
                    .map(|val| decoder::<T>((Cow::Borrowed(encoded_key.as_ref()), val)))
            }
            (None, Some(subkey)) => {
                if let Some((key, _)) = self.first()? {
                    let encoded_key = key.encode();
                    self.inner
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

impl<T: Table> DbCursorRW<T> for Cursor<RW, T> {
    /// Database operation that will update an existing row if a specified value already
    /// exists in a table, and insert a new row if the specified value doesn't already exist
    ///
    /// For a DUPSORT table, `upsert` will not actually update-or-insert. If the key already exists,
    /// it will append the value to the subkey, even if the subkeys are the same. So if you want
    /// to properly upsert, you'll need to `seek_exact` & `delete_current` if the key+subkey was
    /// found, before calling `upsert`.
    fn upsert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();

        // Check if we can use zero-copy reserve path
        if let Some(value_ref) = value.uncompressable_ref() {
            let value_len = value_ref.len();
            return self.execute_with_operation_metric(
                Operation::CursorUpsert,
                Some(value_len),
                |this| {
                    // SAFETY: We fill the reserved buffer immediately and don't use it after
                    unsafe {
                        this.inner
                            .reserve(key.as_ref(), value_len, WriteFlags::UPSERT)
                            .map(|reserved| {
                                reserved.copy_from_slice(value_ref);
                            })
                            .map_err(|e| {
                                DatabaseWriteError {
                                    info: e.into(),
                                    operation: DatabaseWriteOperation::CursorUpsert,
                                    table_name: T::NAME,
                                    key: key.into_vec(),
                                }
                                .into()
                            })
                    }
                },
            );
        }

        // Fallback: compress to buffer then put
        self.buf.clear();
        value.compress_to_buf(&mut self.buf);
        let value_len = self.buf.len();

        self.execute_with_operation_metric(
            Operation::CursorUpsert,
            Some(value_len),
            |this| {
                this.inner
                    .put(key.as_ref(), &this.buf, WriteFlags::UPSERT)
                    .map_err(|e| {
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

        if let Some(value_ref) = value.uncompressable_ref() {
            let value_len = value_ref.len();
            return self.execute_with_operation_metric(
                Operation::CursorInsert,
                Some(value_len),
                |this| {
                    unsafe {
                        this.inner
                            .reserve(key.as_ref(), value_len, WriteFlags::NO_OVERWRITE)
                            .map(|reserved| {
                                reserved.copy_from_slice(value_ref);
                            })
                            .map_err(|e| {
                                DatabaseWriteError {
                                    info: e.into(),
                                    operation: DatabaseWriteOperation::CursorInsert,
                                    table_name: T::NAME,
                                    key: key.into_vec(),
                                }
                                .into()
                            })
                    }
                },
            );
        }

        self.buf.clear();
        value.compress_to_buf(&mut self.buf);
        let value_len = self.buf.len();

        self.execute_with_operation_metric(
            Operation::CursorInsert,
            Some(value_len),
            |this| {
                this.inner
                    .put(key.as_ref(), &this.buf, WriteFlags::NO_OVERWRITE)
                    .map_err(|e| {
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

        if let Some(value_ref) = value.uncompressable_ref() {
            let value_len = value_ref.len();
            return self.execute_with_operation_metric(
                Operation::CursorAppend,
                Some(value_len),
                |this| {
                    unsafe {
                        this.inner
                            .reserve(key.as_ref(), value_len, WriteFlags::APPEND)
                            .map(|reserved| {
                                reserved.copy_from_slice(value_ref);
                            })
                            .map_err(|e| {
                                DatabaseWriteError {
                                    info: e.into(),
                                    operation: DatabaseWriteOperation::CursorAppend,
                                    table_name: T::NAME,
                                    key: key.into_vec(),
                                }
                                .into()
                            })
                    }
                },
            );
        }

        self.buf.clear();
        value.compress_to_buf(&mut self.buf);
        let value_len = self.buf.len();

        self.execute_with_operation_metric(
            Operation::CursorAppend,
            Some(value_len),
            |this| {
                this.inner
                    .put(key.as_ref(), &this.buf, WriteFlags::APPEND)
                    .map_err(|e| {
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
            this.inner.del(WriteFlags::CURRENT).map_err(|e| DatabaseError::Delete(e.into()))
        })
    }
}

impl<T: DupSort> DbDupCursorRW<T> for Cursor<RW, T> {
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        self.execute_with_operation_metric(Operation::CursorDeleteCurrentDuplicates, None, |this| {
            this.inner.del(WriteFlags::NO_DUP_DATA).map_err(|e| DatabaseError::Delete(e.into()))
        })
    }

    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();

        if let Some(value_ref) = value.uncompressable_ref() {
            let value_len = value_ref.len();
            return self.execute_with_operation_metric(
                Operation::CursorAppendDup,
                Some(value_len),
                |this| {
                    unsafe {
                        this.inner
                            .reserve(key.as_ref(), value_len, WriteFlags::APPEND_DUP)
                            .map(|reserved| {
                                reserved.copy_from_slice(value_ref);
                            })
                            .map_err(|e| {
                                DatabaseWriteError {
                                    info: e.into(),
                                    operation: DatabaseWriteOperation::CursorAppendDup,
                                    table_name: T::NAME,
                                    key: key.into_vec(),
                                }
                                .into()
                            })
                    }
                },
            );
        }

        self.buf.clear();
        value.compress_to_buf(&mut self.buf);
        let value_len = self.buf.len();

        self.execute_with_operation_metric(
            Operation::CursorAppendDup,
            Some(value_len),
            |this| {
                this.inner
                    .put(key.as_ref(), &this.buf, WriteFlags::APPEND_DUP)
                    .map_err(|e| {
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
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
        models::{BlockNumberAddress, ClientVersion},
        table::TableImporter,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::StorageEntry;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_db() -> Arc<DatabaseEnv> {
        let path = TempDir::new().unwrap();
        let mut db = DatabaseEnv::open(
            path.path(),
            DatabaseEnvKind::RW,
            DatabaseArguments::new(ClientVersion::default()),
        )
        .unwrap();
        db.create_tables().unwrap();
        Arc::new(db)
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

    /// Tests that the zero-copy reserve path works correctly for B256 values.
    /// B256 implements `uncompressable_ref()` returning Some, so it uses the reserve path.
    #[test]
    fn test_reserve_path_b256_roundtrip() {
        use crate::tables::CanonicalHeaders;

        let db = create_test_db();
        let tx = db.tx_mut().unwrap();

        // B256 values use the reserve path (uncompressable_ref returns Some)
        let test_data: Vec<(u64, B256)> = vec![
            (0, B256::ZERO),
            (1, B256::with_last_byte(1)),
            (100, B256::repeat_byte(0xab)),
            (u64::MAX, B256::repeat_byte(0xff)),
        ];

        // Write using upsert (uses reserve path for B256)
        {
            let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
            for (key, value) in &test_data {
                cursor.upsert(*key, value).unwrap();
            }
        }

        // Read back and verify
        {
            let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
            for (expected_key, expected_value) in &test_data {
                let (key, value) = cursor.seek_exact(*expected_key).unwrap().unwrap();
                assert_eq!(key, *expected_key);
                assert_eq!(value, *expected_value);
            }
        }

        tx.commit().unwrap();

        // Verify again after commit
        let tx = db.tx().unwrap();
        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let all_entries: Vec<_> = cursor.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(all_entries.len(), test_data.len());
        for ((key, value), (expected_key, expected_value)) in all_entries.iter().zip(test_data.iter()) {
            assert_eq!(key, expected_key);
            assert_eq!(value, expected_value);
        }
    }

    /// Tests insert operation with reserve path
    #[test]
    fn test_reserve_path_insert_roundtrip() {
        use crate::tables::CanonicalHeaders;

        let db = create_test_db();
        let tx = db.tx_mut().unwrap();

        let hash1 = B256::repeat_byte(0x11);
        let hash2 = B256::repeat_byte(0x22);

        {
            let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
            cursor.insert(1, &hash1).unwrap();
            cursor.insert(2, &hash2).unwrap();

            // Insert with existing key should fail
            assert!(cursor.insert(1, &hash2).is_err());
        }

        // Verify
        {
            let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
            let (k1, v1) = cursor.seek_exact(1).unwrap().unwrap();
            assert_eq!(k1, 1);
            assert_eq!(v1, hash1);

            let (k2, v2) = cursor.seek_exact(2).unwrap().unwrap();
            assert_eq!(k2, 2);
            assert_eq!(v2, hash2);
        }
    }

    /// Tests append operation with reserve path
    #[test]
    fn test_reserve_path_append_roundtrip() {
        use crate::tables::CanonicalHeaders;

        let db = create_test_db();
        let tx = db.tx_mut().unwrap();

        let hashes: Vec<B256> = (0..10u8).map(|i| B256::repeat_byte(i)).collect();

        {
            let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
            for (i, hash) in hashes.iter().enumerate() {
                cursor.append(i as u64, hash).unwrap();
            }
        }

        // Verify all entries
        {
            let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
            let entries: Vec<_> = cursor.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(entries.len(), hashes.len());
            for (i, (key, value)) in entries.iter().enumerate() {
                assert_eq!(*key, i as u64);
                assert_eq!(*value, hashes[i]);
            }
        }
    }

    /// Tests that DupSort append_dup works with reserve path
    #[test]
    fn test_reserve_path_append_dup_roundtrip() {
        let db = create_test_db();
        let tx = db.tx_mut().unwrap();

        let addr = address!("0000000000000000000000000000000000000001");
        let entries = vec![
            StorageEntry { key: B256::with_last_byte(1), value: U256::from(100) },
            StorageEntry { key: B256::with_last_byte(2), value: U256::from(200) },
            StorageEntry { key: B256::with_last_byte(3), value: U256::from(300) },
        ];

        {
            let mut cursor = tx.cursor_dup_write::<StorageChangeSets>().unwrap();
            for entry in &entries {
                cursor.append_dup(BlockNumberAddress((1, addr)), *entry).unwrap();
            }
        }

        // Verify
        {
            let mut cursor = tx.cursor_dup_read::<StorageChangeSets>().unwrap();
            let results: Vec<_> = cursor
                .walk_dup(Some(BlockNumberAddress((1, addr))), None)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();

            assert_eq!(results.len(), entries.len());
            for ((_, value), expected) in results.iter().zip(entries.iter()) {
                assert_eq!(value, expected);
            }
        }
    }

    /// Tests mixed operations to ensure reserve and non-reserve paths work together
    #[test]
    fn test_reserve_path_mixed_operations() {
        use crate::tables::CanonicalHeaders;

        let db = create_test_db();

        // First transaction: upsert
        {
            let tx = db.tx_mut().unwrap();
            let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
            cursor.upsert(1, &B256::repeat_byte(0x11)).unwrap();
            cursor.upsert(2, &B256::repeat_byte(0x22)).unwrap();
            tx.commit().unwrap();
        }

        // Second transaction: upsert to update existing
        {
            let tx = db.tx_mut().unwrap();
            let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
            cursor.upsert(1, &B256::repeat_byte(0xaa)).unwrap(); // Update
            cursor.upsert(3, &B256::repeat_byte(0x33)).unwrap(); // New
            tx.commit().unwrap();
        }

        // Verify final state
        {
            let tx = db.tx().unwrap();
            let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();

            let (_, v1) = cursor.seek_exact(1).unwrap().unwrap();
            assert_eq!(v1, B256::repeat_byte(0xaa)); // Updated

            let (_, v2) = cursor.seek_exact(2).unwrap().unwrap();
            assert_eq!(v2, B256::repeat_byte(0x22)); // Unchanged

            let (_, v3) = cursor.seek_exact(3).unwrap().unwrap();
            assert_eq!(v3, B256::repeat_byte(0x33)); // New
        }
    }
}
