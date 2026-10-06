//! Cursor wrapper for libmdbx-sys.

use super::utils::*;
use crate::{
    metrics::{Operation, TableOperationMetrics},
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
use std::{borrow::Cow, collections::Bound, marker::PhantomData, ops::RangeBounds};

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
    /// Per-table operation metrics. If `None`, metrics are not recorded.
    metrics: Option<TableOperationMetrics>,
    /// Phantom data to enforce encoding/decoding.
    _dbi: PhantomData<T>,
}

impl<K: TransactionKind, T: Table> Cursor<K, T> {
    pub(crate) const fn new_with_metrics(
        inner: reth_libmdbx::Cursor<K>,
        metrics: Option<TableOperationMetrics>,
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
            metrics[operation.index()].record(value_size, || f(self))
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

    fn seek_by_key_subkey_with<R>(
        &mut self,
        key: T::Key,
        subkey: T::SubKey,
        project: impl FnOnce(&[u8]) -> Result<R, DatabaseError>,
    ) -> Result<Option<R>, DatabaseError> {
        self.inner
            .get_both_range::<Cow<'_, [u8]>>(key.encode().as_ref(), subkey.encode().as_ref())
            .map_err(|e| DatabaseError::Read(e.into()))?
            .map(|value| project(value.as_ref()))
            .transpose()
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
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorUpsert,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                this.inner
                    .put(key.as_ref(), value.unwrap_or(&this.buf), WriteFlags::UPSERT)
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
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorInsert,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                this.inner
                    .put(key.as_ref(), value.unwrap_or(&this.buf), WriteFlags::NO_OVERWRITE)
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
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorAppend,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                this.inner
                    .put(key.as_ref(), value.unwrap_or(&this.buf), WriteFlags::APPEND)
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
        let value = compress_to_buf_or_ref!(self, value);
        self.execute_with_operation_metric(
            Operation::CursorAppendDup,
            Some(value.unwrap_or(&self.buf).len()),
            |this| {
                this.inner
                    .put(key.as_ref(), value.unwrap_or(&this.buf), WriteFlags::APPEND_DUP)
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
    use super::*;
    use crate::{
        mdbx::{DatabaseArguments, DatabaseEnv, DatabaseEnvKind},
        tables::{PackedStoragesTrie, StorageChangeSets, StoragesTrie},
        Database,
    };
    use alloy_primitives::{address, Address, B256, U256};
    use reth_db_api::{
        cursor::{DbCursorRO, DbDupCursorRW},
        models::{BlockNumberAddress, ClientVersion},
        table::TableImporter,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::{StorageEntry, ValueWithSubKey};
    use std::{collections::BTreeMap, hint::black_box, time::Instant};
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

    #[test]
    fn projected_seek_preserves_range_and_cursor_position() {
        let db = create_test_db();
        let key = BlockNumberAddress((100, Address::repeat_byte(1)));
        let values = [2_u8, 4, 6]
            .map(|i| StorageEntry { key: B256::with_last_byte(i), value: U256::from(i) });
        let tx = db.tx_mut().unwrap();
        let mut cursor = tx.cursor_dup_write::<StorageChangeSets>().unwrap();
        for value in values {
            cursor.upsert(key, &value).unwrap();
        }
        for i in 0..=7 {
            let expected = values.iter().find(|value| value.key >= B256::with_last_byte(i));
            assert_eq!(
                cursor
                    .seek_by_key_subkey_with(key, B256::with_last_byte(i), |bytes| Ok(
                        StorageEntry::decompress(bytes)?
                    ))
                    .unwrap(),
                expected.copied(),
            );
            if let Some(value) = expected {
                assert_eq!(cursor.current().unwrap(), Some((key, *value)));
            }
        }
        assert_eq!(
            cursor
                .seek_by_key_subkey_with::<()>(
                    BlockNumberAddress((101, Address::repeat_byte(1))),
                    B256::ZERO,
                    |_| panic!("projection must not run for a missing key"),
                )
                .unwrap(),
            None,
        );
        assert_eq!(
            cursor
                .seek_by_key_subkey_with::<()>(key, B256::with_last_byte(7), |_| {
                    panic!("projection must not run beyond the last subkey")
                })
                .unwrap(),
            None,
        );
        assert!(matches!(
            cursor
                .seek_by_key_subkey_with::<()>(key, values[1].key, |_| Err(DatabaseError::Decode)),
            Err(DatabaseError::Decode),
        ));
        assert_eq!(cursor.current().unwrap(), Some((key, values[1])));
        cursor.delete_current().unwrap();
        drop(cursor);
        tx.commit().unwrap();

        let read = db.tx().unwrap();
        let mut cursor = read.cursor_dup_read::<StorageChangeSets>().unwrap();
        assert_eq!(
            cursor
                .seek_by_key_subkey_with(key, values[1].key, |bytes| Ok(StorageEntry::decompress(
                    bytes
                )?))
                .unwrap(),
            Some(values[2]),
        );
        assert_eq!(
            cursor.walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
            vec![(key, values[0]), (key, values[2])],
        );
    }

    // Compare the public projection API against decoding the complete stored node.
    #[test]
    #[ignore = "paired MDBX storage-trie update timing screen"]
    fn test_storage_trie_seek_without_decode_screen() {
        screen_storage_trie_seek::<StoragesTrie>();
        screen_storage_trie_seek::<PackedStoragesTrie>();
    }

    fn screen_storage_trie_seek<T>()
    where
        T: DupSort<Key = B256>,
        T::SubKey: From<Vec<u8>>,
        T::Value: ValueWithSubKey<SubKey = T::SubKey> + Clone + PartialEq + std::fmt::Debug,
    {
        for duplicates in [16_usize, 512, 4096] {
            for pair in 0..8 {
                let mut elapsed = [0_u128; 2];
                for raw_seek in [pair % 2 == 0, pair % 2 != 0] {
                    let db = create_test_db();
                    let mut expected = BTreeMap::new();
                    let mut updates = Vec::new();
                    // Include prefix-related subkeys and misses on either side of a row.
                    let mut keys: Vec<Vec<u8>> = (0..duplicates)
                        .map(|i| {
                            (0..8)
                                .rev()
                                .map(|shift| (((i * 2) >> (shift * 4)) & 15) as u8)
                                .collect()
                        })
                        .collect();
                    keys.extend([vec![1], vec![1, 0], vec![1, 0, 0], vec![15]]);
                    keys.sort();
                    for round in 0..6_u8 {
                        let mut batch = Vec::new();
                        for account in 1..=4 {
                            let address = B256::repeat_byte(account);
                            for (index, key) in keys.iter().enumerate() {
                                let subkey = T::SubKey::from(key.clone());
                                let value = if round > 0 &&
                                    (index + round as usize).is_multiple_of(11)
                                {
                                    None
                                } else {
                                    // Use the actual trie table codec, varying branch width and
                                    // encoded size across replacements, including a subtree root.
                                    let hashes = 1 + (index + round as usize) % 16;
                                    let mask = ((1_u32 << hashes) - 1) as u16;
                                    let mut bytes = subkey.clone().encode().as_ref().to_vec();
                                    for _ in 0..3 {
                                        bytes.extend(mask.to_be_bytes());
                                    }
                                    bytes.extend(B256::repeat_byte(round ^ account).as_slice());
                                    for hash in 0..hashes {
                                        bytes.extend(
                                            B256::repeat_byte(hash as u8 ^ round).as_slice(),
                                        );
                                    }
                                    let value = T::Value::decompress(&bytes).unwrap();
                                    assert_eq!(value.get_subkey(), subkey);
                                    assert_eq!(value.clone().compress().as_ref(), bytes.as_slice());
                                    Some(value)
                                };
                                batch.push((address, subkey, value));
                            }
                            // These adjacent odd subkeys were not seeded. Deleting one must
                            // leave the next even key and other account rows intact.
                            let absent = T::SubKey::from(vec![0, 0, 0, 0, 0, 0, 0, 1]);
                            batch.push((address, absent, None));
                        }
                        batch.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
                        updates.push(batch);
                    }
                    for (round, batch) in updates.iter().enumerate() {
                        let tx = db.tx_mut().unwrap();
                        let mut cursor = tx.cursor_dup_write::<T>().unwrap();
                        let started = Instant::now();
                        for (address, subkey, value) in black_box(batch) {
                            let found = if raw_seek {
                                cursor
                                    .seek_by_key_subkey_with(*address, subkey.clone(), |bytes| {
                                        let encoded = subkey.clone().encode();
                                        Ok(bytes
                                            .get(..encoded.as_ref().len())
                                            .ok_or(DatabaseError::Decode)? ==
                                            encoded.as_ref())
                                    })
                                    .unwrap()
                                    .unwrap_or(false)
                            } else {
                                cursor
                                    .seek_by_key_subkey(*address, subkey.clone())
                                    .unwrap()
                                    .is_some_and(|value| value.get_subkey() == *subkey)
                            };
                            if found {
                                cursor.delete_current().unwrap();
                            }
                            if let Some(value) = value {
                                cursor.upsert(*address, value).unwrap();
                            }
                        }
                        let update_ns = started.elapsed().as_nanos();
                        drop(cursor);
                        // Check aborted writes as well as committed replacement/deletion batches.
                        if round == 4 {
                            drop(tx);
                        } else {
                            tx.commit().unwrap();
                            for (address, subkey, value) in batch {
                                let key = (*address, subkey.clone());
                                if let Some(value) = value {
                                    expected.insert(key, value.clone());
                                } else {
                                    expected.remove(&key);
                                }
                            }
                        }
                        let read = db.tx().unwrap();
                        let rows = read
                            .cursor_dup_read::<T>()
                            .unwrap()
                            .walk(None)
                            .unwrap()
                            .collect::<Result<Vec<_>, _>>()
                            .unwrap();
                        assert_eq!(rows.len(), expected.len());
                        let actual = rows
                            .into_iter()
                            .map(|(address, value)| ((address, value.get_subkey()), value))
                            .collect::<BTreeMap<_, _>>();
                        assert_eq!(actual, expected);
                        if round > 0 {
                            elapsed[usize::from(raw_seek)] += update_ns;
                        }
                    }
                }
                println!("{{\"table\":\"{}\",\"duplicates\":{},\"pair\":{},\"typed_ns\":{},\"raw_seek_ns\":{},\"time_reduction_pct\":{}}}",
                    std::any::type_name::<T>(), duplicates, pair, elapsed[0], elapsed[1],
                    100.0 * (1.0 - elapsed[1] as f64 / elapsed[0] as f64));
            }
        }
    }
}
