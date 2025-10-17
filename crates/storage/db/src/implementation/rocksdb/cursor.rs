use crate::{
    implementation::rocksdb::{create_write_error, get_cf_handle},
    DatabaseError,
};
use parking_lot::Mutex;
use reth_db_api::{
    common::{IterPairResult, PairResult, ValueOnlyResult},
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, Walker},
    table::{Compress, Decode, Decompress, DupSort, Encode, Table},
};
use reth_storage_errors::db::{DatabaseErrorInfo, DatabaseWriteOperation};
use rocksdb::{MultiThreaded, Transaction, TransactionDB};
use std::{
    fmt::Debug,
    marker::PhantomData,
    mem,
    ops::{Bound, RangeBounds},
    sync::Arc,
};

/// Transaction kind marker for read-only operations.
#[derive(Debug)]
pub struct RO;

/// Transaction kind marker for read-write operations.
#[derive(Debug)]
pub struct RW;

/// Trait to distinguish between read-only and read-write transaction kinds.
pub trait TransactionKind: Debug + Send + Sync + 'static {
    /// Whether this transaction kind is read-write.
    const IS_READ_WRITE: bool;
}

impl TransactionKind for RO {
    const IS_READ_WRITE: bool = false;
}

impl TransactionKind for RW {
    const IS_READ_WRITE: bool = true;
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

/// RocksDB cursor with RawIterator caching for performance.
pub struct Cursor<K: TransactionKind, T: Table> {
    db: Arc<TransactionDB<MultiThreaded>>,
    /// Iterator for cursor operations - always ready to use
    iterator: rocksdb::DBRawIteratorWithThreadMode<'static, TransactionDB<MultiThreaded>>,
    /// Transaction for write operations
    transaction: Arc<Mutex<Transaction<'static, TransactionDB<MultiThreaded>>>>,
    /// Cache buffer that receives compressed values.
    buf: Vec<u8>,
    _phantom: PhantomData<(K, T)>,
}

impl<K: TransactionKind, T: Table> std::fmt::Debug for Cursor<K, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cursor")
            .field("db", &"<TransactionDB>")
            .field("iterator", &"<DBRawIterator>")
            .field("transaction", &"<Transaction>")
            .field("buf_len", &self.buf.len())
            .field("_phantom", &self._phantom)
            .finish()
    }
}

impl<K: TransactionKind, T: Table> Cursor<K, T> {
    const KEY_LENGTH: usize = mem::size_of::<<T::Key as Encode>::Encoded>();

    /// Create cursor with existing transaction
    pub(crate) fn new(
        db: Arc<TransactionDB<MultiThreaded>>,
        transaction: Arc<Mutex<Transaction<'static, TransactionDB<MultiThreaded>>>>,
    ) -> Result<Self, DatabaseError> {
        // Clone db Arc to extend cf_handle lifetime
        let db_for_cf = Arc::clone(&db);
        let cf_handle = get_cf_handle::<T>(&db_for_cf)?;

        // Create iterator using transaction to ensure consistency with writes
        let iterator = {
            let txn = transaction.lock();
            unsafe {
                // SAFETY: We ensure the Transaction outlives the iterator by holding
                // Arc<Mutex<Transaction>>
                std::mem::transmute::<
                    rocksdb::DBRawIteratorWithThreadMode<
                        '_,
                        Transaction<'_, TransactionDB<MultiThreaded>>,
                    >,
                    rocksdb::DBRawIteratorWithThreadMode<'static, TransactionDB<MultiThreaded>>,
                >(txn.raw_iterator_cf(&cf_handle))
            }
        };

        Ok(Self { db, iterator, transaction, buf: Vec::new(), _phantom: PhantomData })
    }

    /// Encode DupSort composite key: key + subkey
    fn encode_dupsort_key(key: &[u8], subkey: &[u8]) -> Vec<u8> {
        let mut composite = Vec::with_capacity(key.len() + subkey.len());
        composite.extend_from_slice(key);
        composite.extend_from_slice(subkey);
        composite
    }

    /// Decode DupSort composite key: split based on fixed key length
    fn decode_dupsort_key(composite: &[u8]) -> Result<(&[u8], &[u8]), DatabaseError> {
        Self::validate_dupsort_key_length(composite)?;
        Ok((&composite[..Self::KEY_LENGTH], &composite[Self::KEY_LENGTH..]))
    }

    /// Validate that a composite key has the correct minimum length for DupSort tables
    fn validate_dupsort_key_length(composite: &[u8]) -> Result<(), DatabaseError> {
        if composite.len() < Self::KEY_LENGTH {
            return Err(DatabaseError::Read(reth_storage_errors::db::DatabaseErrorInfo {
                message: format!(
                    "Invalid DupSort composite key length: expected at least {} bytes, got {} bytes for table '{}'",
                    Self::KEY_LENGTH,
                    composite.len(),
                    T::NAME
                ).into(),
                code: -1,
            }));
        }
        Ok(())
    }

    /// Extract main key from composite key with validation
    fn extract_main_key(composite: &[u8]) -> Result<&[u8], DatabaseError> {
        Self::validate_dupsort_key_length(composite)?;
        Ok(&composite[..Self::KEY_LENGTH])
    }

    fn decode_key_value(key: &[u8], value: &[u8]) -> Result<(T::Key, T::Value), DatabaseError> {
        let decoded_key = if T::DUPSORT {
            // For DupSort tables, key is composite: main_key + subkey
            // Extract only the main key part using fixed length
            T::Key::decode(&key[..Self::KEY_LENGTH]).map_err(|_| DatabaseError::Decode)?
        } else {
            // For regular tables, decode normally
            T::Key::decode(key).map_err(|_| DatabaseError::Decode)?
        };

        let decoded_value = T::Value::decompress(value).map_err(|_| DatabaseError::Decode)?;
        Ok((decoded_key, decoded_value))
    }
}

impl<K: TransactionKind, T: Table> DbCursorRO<T> for Cursor<K, T> {
    fn first(&mut self) -> PairResult<T> {
        self.iterator.seek_to_first();
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn seek_exact(&mut self, key: T::Key) -> PairResult<T> {
        // For seek_exact, we position the iterator at the exact key
        let encoded_key = key.encode();
        self.iterator.seek(encoded_key.as_ref());

        if self.iterator.valid() {
            if let (Some(found_key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                if T::DUPSORT {
                    let main_key = Self::extract_main_key(found_key)?;
                    if main_key == encoded_key.as_ref() {
                        return Self::decode_key_value(found_key, value).map(Some);
                    }
                } else if found_key == encoded_key.as_ref() {
                    return Self::decode_key_value(found_key, value).map(Some);
                }
            }
        }
        Ok(None)
    }

    fn seek(&mut self, key: T::Key) -> PairResult<T> {
        let encoded_key = key.encode();
        self.iterator.seek(encoded_key.as_ref());
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn next(&mut self) -> PairResult<T> {
        self.iterator.next();
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn prev(&mut self) -> PairResult<T> {
        self.iterator.prev();
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn last(&mut self) -> PairResult<T> {
        self.iterator.seek_to_last();
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn current(&mut self) -> PairResult<T> {
        // Get current position from iterator
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn walk(&mut self, start_key: Option<T::Key>) -> Result<Walker<'_, T, Self>, DatabaseError> {
        let start = match start_key {
            Some(key) => self.seek(key).transpose(),
            None => self.first().transpose(),
        };
        Ok(Walker::new(self, start))
    }

    fn walk_range(
        &mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<reth_db_api::cursor::RangeWalker<'_, T, Self>, DatabaseError> {
        let start_key = match range.start_bound() {
            Bound::Included(key) => Some(key.clone()),
            Bound::Excluded(_key) => {
                unreachable!("Rust doesn't allow for Bound::Excluded in starting bounds");
            }
            Bound::Unbounded => None,
        };
        let start = match start_key {
            Some(key) => self.seek(key).transpose(),
            None => self.first().transpose(),
        };

        Ok(reth_db_api::cursor::RangeWalker::new(self, start, range.end_bound().cloned()))
    }

    fn walk_back(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<reth_db_api::cursor::ReverseWalker<'_, T, Self>, DatabaseError> {
        let start: IterPairResult<T> = match start_key {
            Some(key) => {
                self.seek(key)?;
                self.current().transpose()
            }
            None => self.last().transpose(),
        };

        Ok(reth_db_api::cursor::ReverseWalker::new(self, start))
    }
}

impl<T: Table> DbCursorRW<T> for Cursor<RW, T> {
    fn upsert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        let cf_handle = get_cf_handle::<T>(&self.db)?;
        let encoded_key = key.encode();
        let value_ref = compress_to_buf_or_ref!(self, value);
        let compressed_value = value_ref.unwrap_or(&self.buf);

        let transaction = self.transaction.lock();
        if T::DUPSORT {
            let subkey = &compressed_value[..value.subkey_compress_length().unwrap()];
            let composite_key = Self::encode_dupsort_key(encoded_key.as_ref(), subkey);
            transaction.put_cf(&cf_handle, &composite_key, compressed_value).map_err(|e| {
                create_write_error::<T>(e, DatabaseWriteOperation::PutUpsert, encoded_key.into())
            })
        } else {
            transaction.put_cf(&cf_handle, &encoded_key, compressed_value).map_err(|e| {
                create_write_error::<T>(e, DatabaseWriteOperation::PutUpsert, encoded_key.into())
            })
        }
    }

    fn insert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        self.upsert(key, value)
    }

    fn append(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        self.upsert(key, value)
    }

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        // Get current key from iterator
        if self.iterator.valid() {
            if let Some(key) = self.iterator.key() {
                let cf_handle = get_cf_handle::<T>(&self.db)?;

                let transaction = self.transaction.lock();
                transaction.delete_cf(&cf_handle, key).map_err(|e| {
                    create_write_error::<T>(e, DatabaseWriteOperation::PutUpsert, key.to_vec())
                })?;
            }
        }
        Ok(())
    }
}

impl<K: TransactionKind, T: DupSort> DbDupCursorRO<T> for Cursor<K, T> {
    fn next_dup(&mut self) -> PairResult<T> {
        // Get current main key from iterator position
        let current_main_key = if self.iterator.valid() {
            if let Some(current_key) = self.iterator.key() {
                Self::extract_main_key(current_key)?.to_vec()
            } else {
                return self.first();
            }
        } else {
            return self.first();
        };

        // Move to next and check if it's still the same main key
        self.iterator.next();
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                let main_key = Self::extract_main_key(key)?;
                if main_key == &current_main_key {
                    return Self::decode_key_value(key, value).map(Some);
                }
            }
        }
        Ok(None)
    }

    fn next_no_dup(&mut self) -> PairResult<T> {
        // Get the current main key from iterator position
        let mut next_key = if self.iterator.valid() {
            if let Some(current_key) = self.iterator.key() {
                Self::extract_main_key(current_key)?.to_vec()
            } else {
                return self.first();
            }
        } else {
            return self.first();
        };

        // Increment the key to find the next different key
        let mut carry = true;
        for byte in next_key.iter_mut().rev() {
            if carry {
                if *byte == u8::MAX {
                    *byte = 0;
                } else {
                    *byte += 1;
                    carry = false;
                    break;
                }
            }
        }
        if carry {
            // Overflow: no next key possible
            return Ok(None);
        }

        // Seek to the incremented key
        self.iterator.seek(&next_key);
        if self.iterator.valid() {
            if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                return Self::decode_key_value(key, value).map(Some);
            }
        }
        Ok(None)
    }

    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        self.next_dup().map(|result| result.map(|(_, value)| value))
    }

    fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T> {
        let encoded_key = key.encode();
        let encoded_subkey = subkey.encode();
        let composite_key = Self::encode_dupsort_key(encoded_key.as_ref(), encoded_subkey.as_ref());

        // Position iterator at the exact composite key
        self.iterator.seek(&composite_key);
        if self.iterator.valid() {
            if let (Some(found_key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                let main_key = Self::extract_main_key(found_key)?;
                if main_key == encoded_key.as_ref() {
                    let decompressed_value =
                        T::Value::decompress(value).map_err(|_| DatabaseError::Decode)?;
                    return Ok(Some(decompressed_value));
                }
            }
        }
        Ok(None)
    }

    fn walk_dup(
        &mut self,
        key: Option<T::Key>,
        subkey: Option<T::SubKey>,
    ) -> Result<reth_db_api::cursor::DupWalker<'_, T, Self>, DatabaseError> {
        let start = match (key, subkey) {
            (Some(key), Some(subkey)) => {
                let composite_key =
                    Self::encode_dupsort_key(key.encode().as_ref(), subkey.encode().as_ref());
                self.iterator.seek(&composite_key);
                let result = if self.iterator.valid() {
                    if let (Some(key), Some(value)) = (self.iterator.key(), self.iterator.value()) {
                        Self::decode_key_value(key, value).map(Some)
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                };
                result.transpose()
            }
            (Some(key), None) => {
                // Seek to first entry of this key
                self.seek(key).transpose()
            }
            (None, Some(subkey)) => {
                if let Some((key, _)) = self.first()? {
                    return self.walk_dup(Some(key), Some(subkey));
                } else {
                    Some(Err(DatabaseError::Read(DatabaseErrorInfo {
                        message: "Not Found".into(),
                        code: -1,
                    })))
                }
            }
            (None, None) => self.first().transpose(),
        };

        Ok(reth_db_api::cursor::DupWalker { cursor: self, start })
    }
}

impl<T: DupSort> DbDupCursorRW<T> for Cursor<RW, T> {
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        // Get current main key from iterator position
        if self.iterator.valid() {
            if let Some(current_composite_key) = self.iterator.key() {
                let main_key = Self::extract_main_key(current_composite_key)?;

                // Find and delete all entries with the same main key
                let cf_handle = get_cf_handle::<T>(&self.db)?;

                // Create iterator to find all duplicates
                let iter = self.db.iterator_cf(
                    &cf_handle,
                    rocksdb::IteratorMode::From(main_key, rocksdb::Direction::Forward),
                );
                let mut keys_to_delete = Vec::new();

                for result in iter {
                    if let Ok((composite_key, _)) = result {
                        // Validate and extract main key - if invalid, stop processing
                        let found_main_key = Self::extract_main_key(&composite_key)?;
                        if found_main_key == main_key {
                            keys_to_delete.push(composite_key.to_vec());
                        } else {
                            // Different key, stop
                            break;
                        }
                    }
                }

                let transaction = self.transaction.lock();
                for key_to_delete in keys_to_delete {
                    transaction.delete_cf(&cf_handle, &key_to_delete).map_err(|e| {
                        create_write_error::<T>(e, DatabaseWriteOperation::PutUpsert, key_to_delete)
                    })?;
                }
            }
        }
        Ok(())
    }

    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let cf_handle = get_cf_handle::<T>(&self.db)?;
        let encoded_key = key.encode();

        // For DupSort tables, we need to extract the subkey from the value
        // First compress the value and use it as subkey
        let value_ref = compress_to_buf_or_ref!(self, &value);
        let compressed_value = value_ref.unwrap_or(&self.buf);

        let subkey = &compressed_value[..value.subkey_compress_length().unwrap()];
        let composite_key = Self::encode_dupsort_key(encoded_key.as_ref(), subkey);

        let transaction = self.transaction.lock();
        transaction.put_cf(&cf_handle, &composite_key, compressed_value).map_err(|e| {
            create_write_error::<T>(
                e,
                DatabaseWriteOperation::CursorAppendDup,
                composite_key.clone(),
            )
        })
    }
}
