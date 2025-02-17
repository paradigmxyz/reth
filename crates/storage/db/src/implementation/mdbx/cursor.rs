//! Cursor wrapper for libmdbx-sys.
use super::{scalerize_client::ScalerizeClient, SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES, SERIALIZED_HASHED_STORAGES_KEY_BYTES, TABLE_CODE_HASHED_ACCOUNTS, TABLE_CODE_HASHED_STORAGES};
use crate::{
    metrics::{DatabaseEnvMetrics, Operation},
    tables::utils::*,
    DatabaseError,
};
use alloy_primitives::B256;
use reth_primitives::{Account, StorageEntry};
use reth_db_api::{
    common::{PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    table::{Compress, Decode, Decompress, DupSort, Encode, Table},
};
use reth_libmdbx::{Error as MDBXError, TransactionKind, WriteFlags, RO, RW};
use reth_storage_errors::db::{DatabaseErrorInfo, DatabaseWriteError, DatabaseWriteOperation};
use std::{borrow::Cow, collections::Bound, marker::PhantomData, ops::RangeBounds, sync::{Arc, RwLock}};
use uuid::Uuid;

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
    // Client for making DB calls to scalerize
    scalerize_client: Arc<RwLock<ScalerizeClient>>,
    /// An 8-byte id generated from a UUID.
    id: [u8; 8],
}

impl<K: TransactionKind, T: Table> Cursor<K, T> {
    pub(crate) fn new_with_metrics(
        inner: reth_libmdbx::Cursor<K>,
        metrics: Option<Arc<DatabaseEnvMetrics>>,
        scalerize_client: Arc<RwLock<ScalerizeClient>>,
    ) -> Self {
        let uuid = Uuid::new_v4();
        let mut id = [0u8; 8];
        id.copy_from_slice(&uuid.as_bytes()[..8]);
        Self {inner, buf: Vec::new(), metrics, _dbi: PhantomData, scalerize_client: scalerize_client, id}
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
#[allow(clippy::type_complexity)]
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
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let response = client.first(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let response = client.first(code, self.id.to_vec(), SERIALIZED_HASHED_STORAGES_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                    }
                _ => unreachable!(),
            }
        }

        decode::<T>(self.inner.first())
    }

    fn seek_exact(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let serialized_key = bincode::serialize(&key).map_err(|_| {
                DatabaseError::Other("Failed to serialize key".to_string())
            })?;
            let response = client.seek_exact(code, self.id.to_vec(), &serialized_key).map_err(DatabaseError::from)?;
            if response.is_none() {
                return Ok(None)
            }

            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let account: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;
                    unsafe {
                        let ptr = &account as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let storage_entry: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;
                    unsafe {
                        let ptr = &storage_entry as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                _ => unreachable!(),
            }
        }

        decode::<T>(self.inner.set_key(key.encode().as_ref()))
    }

    fn seek(&mut self, key: <T as Table>::Key) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let serialized_key = bincode::serialize(&key).map_err(|_| {
                DatabaseError::Other("Failed to serialize key".to_string())
            })?;
            let response = client.seek(code, self.id.to_vec(), &serialized_key).map_err(DatabaseError::from)?;
            if response.is_none() {
                return Ok(None)
            }
            
            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let account: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;
                    unsafe {
                        let ptr = &account as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let storage_entry: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;
                    unsafe {
                        let ptr = &storage_entry as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                _ => unreachable!(),
            }
        }
        decode::<T>(self.inner.set_range(key.encode().as_ref()))
    }

    fn next(&mut self) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let response = client.next(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let response = client.next(code, self.id.to_vec(), SERIALIZED_HASHED_STORAGES_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                _ => unreachable!(),
            }
        }

        decode::<T>(self.inner.next())
    }

    fn prev(&mut self) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let response = client.prev(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let response = client.prev(code, self.id.to_vec(), SERIALIZED_HASHED_STORAGES_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                _ => unreachable!(),
            }
        }

        decode::<T>(self.inner.prev())
    }

    fn last(&mut self) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let response = client.last(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let response = client.last(code, self.id.to_vec(), SERIALIZED_HASHED_STORAGES_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                _ => unreachable!(),
            }
        }
        decode::<T>(self.inner.last())
    }

    fn current(&mut self) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            match code {
                TABLE_CODE_HASHED_ACCOUNTS => {
                    let response = client.current(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: Account = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize Account".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const Account as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                TABLE_CODE_HASHED_STORAGES => {
                    let response = client.current(code, self.id.to_vec(), SERIALIZED_HASHED_STORAGES_KEY_BYTES).map_err(DatabaseError::from)?;
                    if response.is_none() {
                        return Ok(None)
                    }

                    let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
                    let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

                    unsafe {
                        let ptr = &key as *const B256 as *const <T as Table>::Key;
                        let key = ptr.read();

                        let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                        let value = ptr.read();

                        return Ok(Some((key, value)))
                    }
                }
                _ => unreachable!(),
            }
        }
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
    /// Returns the next `(key, value)` pair of a DUPSORT table.
    fn next_dup(&mut self) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let response = client.next_dup(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
            if response.is_none() {
                return Ok(None)
            }

            let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
            let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

            unsafe {
                let ptr = &key as *const B256 as *const <T as Table>::Key;
                let key = ptr.read();

                let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                let value = ptr.read();

                return Ok(Some((key, value)))
            } 
        }

        decode::<T>(self.inner.next_dup())
    }

    /// Returns the next `(key, value)` pair skipping the duplicates.
    fn next_no_dup(&mut self) -> PairResult<T> {
        let table_code = match T::NAME {
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let response = client.next_no_dup(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
            if response.is_none() {
                return Ok(None)
            }

            let key: B256 = bincode::deserialize(&response.as_ref().unwrap().0).map_err(|_| {DatabaseError::Other("Failed to deserialize Key".to_string())})?;
            let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

            unsafe {
                let ptr = &key as *const B256 as *const <T as Table>::Key;
                let key = ptr.read();

                let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                let value = ptr.read();

                return Ok(Some((key, value)))
            } 
        }

        decode::<T>(self.inner.next_nodup())
    }

    /// Returns the next `value` of a duplicate `key`.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        let table_code = match T::NAME {
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let response = client.next_dup(code, self.id.to_vec(), SERIALIZED_HASHED_ACCOUNTS_KEY_BYTES).map_err(DatabaseError::from)?;
            if response.is_none() {
                return Ok(None)
            }

            let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap().1).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

            unsafe {
                let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                let value = ptr.read();

                return Ok(Some(value))
            } 
        }

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
        let table_code = match T::NAME {
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let key = bincode::serialize(&key).map_err(|_| DatabaseError::Other("Failed to serialize Key".to_string()))?;
            let subkey = bincode::serialize(&subkey).map_err(|_| DatabaseError::Other("Failed to serialize Subkey".to_string()))?;

            let response = client.seek_by_key_subkey(code, self.id.to_vec(), &key, &subkey).map_err(DatabaseError::from)?;
            if response.is_none() {
                return Ok(None)
            }
            
            let value: StorageEntry = bincode::deserialize(&response.as_ref().unwrap()).map_err(|_| {DatabaseError::Other("Failed to deserialize StorageEntry".to_string())})?;

            unsafe {
                let ptr = &value as *const StorageEntry as *const <T as Table>::Value;
                let value = ptr.read();

                return Ok(Some(value))
            } 
        }

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
                // encode key and decode it after.
                let key: Vec<u8> = key.encode().into();
                self.inner
                    .get_both_range(key.as_ref(), subkey.encode().as_ref())
                    .map_err(|e| DatabaseError::Read(e.into()))?
                    .map(|val| decoder::<T>((Cow::Owned(key), val)))
            }
            (Some(key), None) => {
                let key: Vec<u8> = key.encode().into();
                self.inner
                    .set(key.as_ref())
                    .map_err(|e| DatabaseError::Read(e.into()))?
                    .map(|val| decoder::<T>((Cow::Owned(key), val)))
            }
            (None, Some(subkey)) => {
                if let Some((key, _)) = self.first()? {
                    let key: Vec<u8> = key.encode().into();
                    self.inner
                        .get_both_range(key.as_ref(), subkey.encode().as_ref())
                        .map_err(|e| DatabaseError::Read(e.into()))?
                        .map(|val| decoder::<T>((Cow::Owned(key), val)))
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
    fn upsert(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let key = bincode::serialize(&key).map_err(|_| DatabaseError::Other("Failed to serialize Key".to_string()))?;
            let value = bincode::serialize(&value).map_err(|_| {DatabaseError::Other("Failed to serialize Value".to_string())})?;
            client.upsert(code, self.id.to_vec(), key.as_slice(), &value).map_err(DatabaseError::from)?;
            return client.write().map_err(DatabaseError::from)
        }

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
                            key: key.into(),
                        }
                        .into()
                    })
            },
        )
    }

    fn insert(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let key = bincode::serialize(&key).map_err(|_| DatabaseError::Other("Failed to serialize Key".to_string()))?;
            let value = bincode::serialize(&value).map_err(|_| {DatabaseError::Other("Failed to serialize Value".to_string())})?;
            client.insert(code, self.id.to_vec(), key.as_slice(), &value).map_err(DatabaseError::from)?;
            return client.write().map_err(DatabaseError::from)
        }

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
                            key: key.into(),
                        }
                        .into()
                    })
            },
        )
    }

    /// Appends the data to the end of the table. Consequently, the append operation
    /// will fail if the inserted key is less than the last table key
    fn append(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            let key = bincode::serialize(&key).map_err(|_| DatabaseError::Other("Failed to serialize Key".to_string()))?;
            let value = bincode::serialize(&value).map_err(|_| {DatabaseError::Other("Failed to serialize Value".to_string())})?;
            client.append(code, self.id.to_vec(), key.as_slice(), &value).map_err(DatabaseError::from)?;
            return client.write().map_err(DatabaseError::from)
        }

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
                            key: key.into(),
                        }
                        .into()
                    })
            },
        )
    }

    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        let table_code = match T::NAME {
            "HashedAccounts" => Some(TABLE_CODE_HASHED_ACCOUNTS),
            "HashedStorages" => Some(TABLE_CODE_HASHED_STORAGES),
            _ => None,
        };

        if let Some(code) = table_code {
            let mut client = self.scalerize_client.write().map_err(|e| DatabaseError::Other(e.to_string()))?;
            client.delete_current(code, self.id.to_vec()).map_err(DatabaseError::from)?;
            return client.write().map_err(DatabaseError::from)
        }

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
                            key: key.into(),
                        }
                        .into()
                    })
            },
        )
    }
}
