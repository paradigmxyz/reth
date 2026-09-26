//! Experimental native Monad `TrieDB` storage for Reth's account and storage tables.

use crate::DatabaseError;
use libloading::Library;
use parking_lot::Mutex;
use reth_db_api::{
    common::{PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    table::{Compress, Decode, Decompress, DupSort, Encode, Table},
};
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CStr, CString},
    fmt,
    marker::PhantomData,
    ops::{Bound, RangeBounds},
    path::Path,
    ptr,
    sync::Arc,
};

const KEY_LEN: usize = 100;
const EMPTY_VERSION: u64 = u64::MAX;
type Key = [u8; KEY_LEN];
type Row = (Key, Vec<u8>);

#[derive(Clone, Copy, Debug)]
pub(crate) struct TableSpec {
    id: u8,
    key_len: usize,
    dup: bool,
}

impl TableSpec {
    pub(crate) fn named(name: &str) -> Option<Self> {
        let (id, key_len, dup) = match name {
            "PlainAccountState" => (1, 20, false),
            "PlainStorageState" => (2, 20, true),
            "HashedAccounts" => (3, 32, false),
            "HashedStorages" => (4, 32, true),
            _ => return None,
        };
        Some(Self { id, key_len, dup })
    }

    fn key(self, key: &[u8], suffix: &[u8]) -> Result<Key, DatabaseError> {
        if key.len() != self.key_len || suffix.len() > 65 {
            return Err(error("invalid state key length"));
        }
        let mut out = [0; KEY_LEN];
        out[0] = self.id;
        out[1..1 + key.len()].copy_from_slice(key);
        out[33..33 + suffix.len()].copy_from_slice(suffix);
        out[99] = suffix.len() as u8;
        Ok(out)
    }

    fn bound(self, key: &[u8], suffix: &[u8]) -> Result<Key, DatabaseError> {
        let mut bound = self.key(key, suffix)?;
        bound[99] = 0;
        Ok(bound)
    }

    fn primary(self, key: &Key) -> &[u8] {
        &key[1..1 + self.key_len]
    }
    const fn low(self) -> Key {
        let mut key = [0; KEY_LEN];
        key[0] = self.id;
        key
    }
    const fn high(self) -> Key {
        let mut key = [255; KEY_LEN];
        key[0] = self.id;
        key
    }
}

fn error(message: impl Into<String>) -> DatabaseError {
    DatabaseError::Other(format!("Monad TrieDB: {}", message.into()))
}

#[repr(C)]
struct Update {
    key: *const u8,
    value: *const u8,
    len: usize,
    deleted: bool,
}

struct Api {
    _library: Library,
    open: unsafe extern "C" fn(*const c_char, bool, bool, *mut *mut c_void) -> i32,
    close: unsafe extern "C" fn(*mut c_void),
    error: unsafe extern "C" fn() -> *const c_char,
    get: unsafe extern "C" fn(*mut c_void, u64, *const u8, *mut *mut c_void) -> i32,
    seek: unsafe extern "C" fn(*mut c_void, u64, *const u8, bool, *mut *mut c_void) -> i32,
    commit: unsafe extern "C" fn(*mut c_void, u64, *const Update, usize, *mut u64) -> i32,
    record_key: unsafe extern "C" fn(*mut c_void) -> *const u8,
    record_value: unsafe extern "C" fn(*mut c_void, *mut usize) -> *const u8,
    record_free: unsafe extern "C" fn(*mut c_void),
}

impl Api {
    fn load() -> Result<Self, DatabaseError> {
        let path = std::env::var_os("RETH_TRIEDB_LIBRARY")
            .ok_or_else(|| error("RETH_TRIEDB_LIBRARY is required"))?;
        // SAFETY: The operator explicitly selects the native adapter library. Symbols have the
        // C ABI defined in native/triedb/bridge.cpp and remain loaded for every handle's lifetime.
        unsafe {
            let library = Library::new(path).map_err(|e| error(e.to_string()))?;
            let abi = library
                .get::<unsafe extern "C" fn() -> u32>(b"reth_triedb_abi_version\0")
                .map_err(|e| error(e.to_string()))?;
            if abi() != 1 {
                return Err(error("incompatible native adapter ABI"));
            }
            macro_rules! symbol {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| error(e.to_string()))?
                };
            }
            Ok(Self {
                open: symbol!("reth_triedb_open"),
                close: symbol!("reth_triedb_close"),
                error: symbol!("reth_triedb_error"),
                get: symbol!("reth_triedb_get"),
                seek: symbol!("reth_triedb_seek"),
                commit: symbol!("reth_triedb_commit"),
                record_key: symbol!("reth_triedb_record_key"),
                record_value: symbol!("reth_triedb_record_value"),
                record_free: symbol!("reth_triedb_record_free"),
                _library: library,
            })
        }
    }

    fn check(&self, code: i32) -> Result<i32, DatabaseError> {
        if code >= 0 {
            return Ok(code);
        }
        // SAFETY: The native error is a null-terminated thread-local string, read before any
        // subsequent native operation can change it.
        Err(error(unsafe { CStr::from_ptr((self.error)()) }.to_string_lossy()))
    }

    fn record(&self, raw: *mut c_void) -> Row {
        // SAFETY: Successful get/seek returns one owned Record. Copy its buffers before freeing it.
        unsafe {
            let mut key = [0; KEY_LEN];
            ptr::copy_nonoverlapping((self.record_key)(raw), key.as_mut_ptr(), KEY_LEN);
            let mut len = 0;
            let value = (self.record_value)(raw, &raw mut len);
            let value =
                if len == 0 { Vec::new() } else { std::slice::from_raw_parts(value, len).to_vec() };
            (self.record_free)(raw);
            (key, value)
        }
    }
}

pub(crate) struct Store {
    api: Api,
    handle: *mut c_void,
}

// SAFETY: Native readers use Monad's concurrent RODb service; writes and ordered traversal are
// serialized by the native adapter. Every operation keeps the owning Store alive.
unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MonadTrieDB").finish_non_exhaustive()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: This is the sole owner of the native handle and all transactions own an Arc.
        unsafe { (self.api.close)(self.handle) };
    }
}

impl Store {
    pub(crate) fn open(
        path: &Path,
        create: bool,
        readonly: bool,
    ) -> Result<Arc<Self>, DatabaseError> {
        if !create && !path.try_exists().map_err(|e| error(e.to_string()))? {
            return Err(error(format!("database device is missing: {}", path.display())));
        }
        let api = Api::load()?;
        let path =
            CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| error(e.to_string()))?;
        let mut handle = ptr::null_mut();
        // SAFETY: The path and output pointer are valid for the duration of the call.
        api.check(unsafe { (api.open)(path.as_ptr(), create, readonly, &raw mut handle) })?;
        Ok(Arc::new(Self { api, handle }))
    }

    fn get(&self, version: u64, key: &Key) -> Result<Option<Row>, DatabaseError> {
        if version == EMPTY_VERSION {
            return Ok(None);
        }
        let mut out = ptr::null_mut();
        // SAFETY: The handle is alive and the fixed-size key/output buffers match the native ABI.
        let code = self
            .api
            .check(unsafe { (self.api.get)(self.handle, version, key.as_ptr(), &raw mut out) })?;
        Ok((code == 1).then(|| self.api.record(out)))
    }

    fn seek(&self, version: u64, key: &Key, reverse: bool) -> Result<Option<Row>, DatabaseError> {
        if version == EMPTY_VERSION {
            return Ok(None);
        }
        let mut out = ptr::null_mut();
        // SAFETY: The handle is alive and the fixed-size key/output buffers match the native ABI.
        let code = self.api.check(unsafe {
            (self.api.seek)(self.handle, version, key.as_ptr(), reverse, &raw mut out)
        })?;
        Ok((code == 1).then(|| self.api.record(out)))
    }
}

#[derive(Debug)]
pub(crate) struct Transaction {
    store: Arc<Store>,
    base: u64,
    writable: bool,
    changes: Mutex<BTreeMap<Key, Option<Vec<u8>>>>,
}

impl Transaction {
    pub(crate) fn new(store: Arc<Store>, base: u64, writable: bool) -> Arc<Self> {
        Arc::new(Self { store, base, writable, changes: Mutex::new(BTreeMap::new()) })
    }

    fn exact(&self, key: &Key) -> Result<Option<Row>, DatabaseError> {
        if self.writable &&
            let Some(value) = self.changes.lock().get(key)
        {
            return Ok(value.as_ref().map(|value| (*key, value.clone())));
        }
        self.store.get(self.base, key)
    }

    fn seek(&self, mut bound: Key, reverse: bool) -> Result<Option<Row>, DatabaseError> {
        if !self.writable {
            return self.store.seek(self.base, &bound, reverse);
        }
        let changes = self.changes.lock();
        let pending = if reverse {
            changes.range(..=bound).rev().find_map(|(k, v)| v.as_ref().map(|v| (*k, v.clone())))
        } else {
            changes.range(bound..).find_map(|(k, v)| v.as_ref().map(|v| (*k, v.clone())))
        };
        let mut stored = self.store.seek(self.base, &bound, reverse)?;
        while let Some((key, _)) = &stored {
            if let Some(value) = changes.get(key) {
                if let Some(value) = value {
                    stored = Some((*key, value.clone()));
                    break;
                }
                bound = *key;
                if !step(&mut bound, reverse) {
                    stored = None;
                    break;
                }
                stored = self.store.seek(self.base, &bound, reverse)?;
            } else {
                break;
            }
        }
        Ok(match (stored, pending) {
            (Some(a), Some(b)) => Some(if (a.0 < b.0) == reverse { b } else { a }),
            (a, b) => a.or(b),
        })
    }

    fn change(&self, key: Key, value: Option<Vec<u8>>) -> Result<(), DatabaseError> {
        if !self.writable {
            return Err(error("write through read-only transaction"));
        }
        let mut changes = self.changes.lock();
        let existed = if let Some(previous) = changes.get(&key) {
            previous.is_some()
        } else {
            self.store.get(self.base, &key)?.is_some()
        };
        if existed != value.is_some() {
            let count_key = count_key(key[0]);
            let previous = if let Some(value) = changes.get(&count_key) {
                value.clone()
            } else {
                self.store.get(self.base, &count_key)?.map(|(_, value)| value)
            };
            let count = decode_count(previous)?;
            let count = if existed { count.checked_sub(1) } else { count.checked_add(1) }
                .ok_or_else(|| error("table count overflow or underflow"))?;
            changes.insert(count_key, Some(count.to_be_bytes().to_vec()));
        }
        changes.insert(key, value);
        Ok(())
    }

    pub(crate) fn put(
        &self,
        table: TableSpec,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), DatabaseError> {
        self.change(table.key(key, if table.dup { value } else { &[] })?, Some(value.to_vec()))
    }

    /// Imports unique source records while initializing a new native database. The caller holds
    /// the MDBX writer transaction and visits every source record exactly once, so checking each
    /// record's absence in the native database would add unnecessary I/O during bulk migration.
    pub(crate) fn import(
        &self,
        table: TableSpec,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), DatabaseError> {
        let key = table.key(key, if table.dup { value } else { &[] })?;
        let mut changes = self.changes.lock();
        if changes.insert(key, Some(value.to_vec())).is_some() {
            return Err(error("duplicate record in bulk import"));
        }
        let count_key = count_key(table.id);
        let previous = if let Some(value) = changes.get(&count_key) {
            value.clone()
        } else {
            self.store.get(self.base, &count_key)?.map(|(_, value)| value)
        };
        let count =
            decode_count(previous)?.checked_add(1).ok_or_else(|| error("table count overflow"))?;
        changes.insert(count_key, Some(count.to_be_bytes().to_vec()));
        Ok(())
    }

    pub(crate) fn append(
        &self,
        table: TableSpec,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), DatabaseError> {
        if let Some((last, _)) = self.seek(table.high(), true)? &&
            last[0] == table.id &&
            table.primary(&last) > key
        {
            return Err(error("append key is not after the table's last key"));
        }
        self.put(table, key, value)
    }

    pub(crate) fn get(
        &self,
        table: TableSpec,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, DatabaseError> {
        let row = if table.dup {
            self.seek(table.bound(key, &[])?, false)?
        } else {
            self.exact(&table.key(key, &[])?)?
        };
        Ok(row.filter(|(k, _)| k[0] == table.id && table.primary(k) == key).map(|(_, v)| v))
    }

    pub(crate) fn delete(
        &self,
        table: TableSpec,
        key: &[u8],
        value: Option<&[u8]>,
    ) -> Result<bool, DatabaseError> {
        if let Some(value) = value {
            let raw = table.key(key, if table.dup { value } else { &[] })?;
            if self.exact(&raw)?.is_some_and(|(_, v)| v == value) {
                self.change(raw, None)?;
                return Ok(true);
            }
            return Ok(false);
        }
        let mut removed = false;
        let mut bound = table.bound(key, &[])?;
        while let Some((raw, _)) = self.seek(bound, false)? {
            if raw[0] != table.id || table.primary(&raw) != key {
                break;
            }
            self.change(raw, None)?;
            removed = true;
            bound = raw;
            if !step(&mut bound, false) {
                break;
            }
        }
        Ok(removed)
    }

    pub(crate) fn clear(&self, table: TableSpec) -> Result<(), DatabaseError> {
        let mut bound = table.low();
        while let Some((key, _)) = self.seek(bound, false)? {
            if key[0] != table.id {
                break;
            }
            self.change(key, None)?;
            bound = key;
            if !step(&mut bound, false) {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn entries(&self, table: TableSpec) -> Result<usize, DatabaseError> {
        let value = self.exact(&count_key(table.id))?.map(|(_, value)| value);
        usize::try_from(decode_count(value)?).map_err(|e| error(e.to_string()))
    }

    pub(crate) fn commit(&self) -> Result<u64, DatabaseError> {
        let mut changes = self.changes.lock();
        if changes.is_empty() && self.base != EMPTY_VERSION {
            return Ok(self.base);
        }
        // A permanent sentinel ensures an empty state still has a versioned trie root.
        changes.insert([0; KEY_LEN], Some(vec![0]));
        let updates = changes
            .iter()
            .map(|(key, value)| Update {
                key: key.as_ptr(),
                value: value.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
                len: value.as_ref().map_or(0, Vec::len),
                deleted: value.is_none(),
            })
            .collect::<Vec<_>>();
        let mut version = EMPTY_VERSION;
        // SAFETY: The locked change set owns all buffers for the synchronous native commit.
        self.store.api.check(unsafe {
            (self.store.api.commit)(
                self.store.handle,
                self.base,
                updates.as_ptr(),
                updates.len(),
                &raw mut version,
            )
        })?;
        Ok(version)
    }
}

const fn count_key(table: u8) -> Key {
    let mut key = [0; KEY_LEN];
    key[1] = table;
    key
}

fn decode_count(value: Option<Vec<u8>>) -> Result<u64, DatabaseError> {
    value.map_or(Ok(0), |value| {
        Ok(u64::from_be_bytes(value.try_into().map_err(|_| error("invalid table count"))?))
    })
}

fn step(key: &mut Key, reverse: bool) -> bool {
    for byte in key.iter_mut().rev() {
        let (value, overflow) =
            if reverse { byte.overflowing_sub(1) } else { byte.overflowing_add(1) };
        *byte = value;
        if !overflow {
            return true;
        }
    }
    false
}

#[derive(Debug)]
pub(crate) struct Cursor<T: Table> {
    tx: Arc<Transaction>,
    table: TableSpec,
    position: Option<Key>,
    forward_eof: bool,
    reverse_eof: bool,
    duplicate_eof: bool,
    _table: PhantomData<T>,
}

impl<T: Table> Cursor<T> {
    pub(crate) const fn new(tx: Arc<Transaction>, table: TableSpec) -> Self {
        Self {
            tx,
            table,
            position: None,
            forward_eof: false,
            reverse_eof: false,
            duplicate_eof: false,
            _table: PhantomData,
        }
    }

    fn decode(&mut self, row: Option<Row>) -> PairResult<T> {
        let Some((key, value)) = row.filter(|(k, _)| k[0] == self.table.id) else {
            return Ok(None);
        };
        self.position = Some(key);
        self.forward_eof = false;
        self.reverse_eof = false;
        self.duplicate_eof = false;
        Ok(Some((T::Key::decode(self.table.primary(&key))?, T::Value::decompress(&value)?)))
    }

    fn seek_raw(&mut self, key: Key, reverse: bool) -> PairResult<T> {
        let row = self.tx.seek(key, reverse)?.filter(|(key, _)| key[0] == self.table.id);
        if row.is_none() {
            if reverse {
                self.reverse_eof = true;
            } else {
                self.forward_eof = true;
            }
        }
        self.decode(row)
    }
}

impl<T: Table> DbCursorRO<T> for Cursor<T> {
    fn first(&mut self) -> PairResult<T> {
        self.seek_raw(self.table.low(), false)
    }
    fn last(&mut self) -> PairResult<T> {
        self.seek_raw(self.table.high(), true)
    }
    fn seek(&mut self, key: T::Key) -> PairResult<T> {
        self.seek_raw(self.table.bound(key.encode().as_ref(), &[])?, false)
    }
    fn seek_exact(&mut self, key: T::Key) -> PairResult<T> {
        let encoded = key.encode();
        let row = if self.table.dup {
            self.tx.seek(self.table.bound(encoded.as_ref(), &[])?, false)?
        } else {
            self.tx.exact(&self.table.key(encoded.as_ref(), &[])?)?
        }
        .filter(|(k, _)| k[0] == self.table.id && self.table.primary(k) == encoded.as_ref());
        if row.is_none() {
            self.forward_eof = true;
            self.duplicate_eof = true;
        }
        self.decode(row)
    }
    fn next(&mut self) -> PairResult<T> {
        if self.forward_eof {
            return Ok(None);
        }
        let Some(mut key) = self.position else {
            return self.first();
        };
        if !step(&mut key, false) {
            return Ok(None);
        }
        self.seek_raw(key, false)
    }
    fn prev(&mut self) -> PairResult<T> {
        if self.reverse_eof {
            return Ok(None);
        }
        let Some(mut key) = self.position else {
            return self.last();
        };
        if !step(&mut key, true) {
            return Ok(None);
        }
        self.seek_raw(key, true)
    }
    fn current(&mut self) -> PairResult<T> {
        let Some(key) = self.position else {
            return Ok(None);
        };
        self.decode(self.tx.exact(&key)?)
    }
    fn walk(&mut self, start: Option<T::Key>) -> Result<Walker<'_, T, Self>, DatabaseError> {
        let start = if let Some(key) = start { self.seek(key) } else { self.first() }.transpose();
        Ok(Walker::new(self, start))
    }
    fn walk_back(
        &mut self,
        start: Option<T::Key>,
    ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError> {
        let start = if let Some(key) = start { self.seek(key) } else { self.last() }.transpose();
        Ok(ReverseWalker::new(self, start))
    }
    fn walk_range(
        &mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'_, T, Self>, DatabaseError> {
        let start = match range.start_bound() {
            Bound::Included(key) => self.seek(key.clone()),
            Bound::Unbounded => self.first(),
            Bound::Excluded(_) => return Err(error("excluded range starts are unsupported")),
        }
        .transpose();
        Ok(RangeWalker::new(self, start, range.end_bound().cloned()))
    }
}

impl<T: DupSort> DbDupCursorRO<T> for Cursor<T> {
    fn next_dup(&mut self) -> PairResult<T> {
        if self.duplicate_eof {
            return Ok(None);
        }
        let Some(current) = self.position else {
            return Ok(None);
        };
        let mut bound = current;
        if !step(&mut bound, false) {
            return Ok(None);
        }
        let row = self.tx.seek(bound, false)?.filter(|(k, _)| {
            k[0] == self.table.id && self.table.primary(k) == self.table.primary(&current)
        });
        self.duplicate_eof = row.is_none();
        self.decode(row)
    }
    fn prev_dup(&mut self) -> PairResult<T> {
        let Some(current) = self.position else {
            return Ok(None);
        };
        let mut bound = current;
        if !step(&mut bound, true) {
            return Ok(None);
        }
        self.decode(
            self.tx
                .seek(bound, true)?
                .filter(|(k, _)| self.table.primary(k) == self.table.primary(&current)),
        )
    }
    fn last_dup(&mut self) -> ValueOnlyResult<T> {
        let Some(current) = self.position else {
            return Ok(None);
        };
        let mut bound = current;
        bound[33..].fill(255);
        Ok(self.seek_raw(bound, true)?.map(|(_, value)| value))
    }
    fn next_no_dup(&mut self) -> PairResult<T> {
        if self.forward_eof {
            return Ok(None);
        }
        let Some(mut bound) = self.position else {
            return self.first();
        };
        bound[33..].fill(255);
        if !step(&mut bound, false) {
            return Ok(None);
        }
        self.seek_raw(bound, false)
    }
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        Ok(self.next_dup()?.map(|(_, value)| value))
    }
    fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T> {
        let key = key.encode();
        let bound = self.table.bound(key.as_ref(), subkey.encode().as_ref())?;
        let row =
            self.tx.seek(bound, false)?.filter(|(k, _)| self.table.primary(k) == key.as_ref());
        self.duplicate_eof = row.is_none();
        Ok(self.decode(row)?.map(|(_, value)| value))
    }
    fn walk_dup(
        &mut self,
        key: Option<T::Key>,
        subkey: Option<T::SubKey>,
    ) -> Result<DupWalker<'_, T, Self>, DatabaseError> {
        let key = match key {
            Some(key) => Some(key),
            None => self.first()?.map(|(key, _)| key),
        };
        let start = if let Some(key) = key {
            if let Some(subkey) = subkey {
                self.seek_by_key_subkey(key.clone(), subkey)?.map(|value| Ok((key, value)))
            } else {
                self.seek_exact(key).transpose()
            }
        } else {
            None
        };
        Ok(DupWalker { cursor: self, start })
    }
}

impl<T: Table> DbCursorRW<T> for Cursor<T> {
    fn upsert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        let mut compressed = Vec::new();
        value.compress_to_buf(&mut compressed);
        let value = compressed;
        self.tx.put(self.table, key.as_ref(), value.as_ref())?;
        self.position =
            Some(self.table.key(key.as_ref(), if self.table.dup { value.as_ref() } else { &[] })?);
        self.forward_eof = false;
        self.reverse_eof = false;
        self.duplicate_eof = false;
        Ok(())
    }
    fn insert(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        if self.tx.get(self.table, key.clone().encode().as_ref())?.is_some() {
            return Err(error("key already exists"));
        }
        self.upsert(key, value)
    }
    fn append(&mut self, key: T::Key, value: &T::Value) -> Result<(), DatabaseError> {
        if let Some((last, _)) = self.tx.seek(self.table.high(), true)? &&
            last[0] == self.table.id &&
            self.table.primary(&last) > key.clone().encode().as_ref()
        {
            return Err(error("append key is not after the table's last key"));
        }
        self.upsert(key, value)
    }
    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        if let Some(key) = self.position {
            self.tx.change(key, None)?;
        }
        Ok(())
    }
}

impl<T: DupSort> DbDupCursorRW<T> for Cursor<T> {
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        if let Some(key) = self.position {
            self.tx.delete(self.table, self.table.primary(&key), None)?;
        }
        Ok(())
    }
    fn append_dup(&mut self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let mut compressed = Vec::new();
        value.compress_to_buf(&mut compressed);
        let raw = self.table.key(key.clone().encode().as_ref(), &compressed)?;
        let mut bound = raw;
        bound[33..].fill(255);
        if let Some((last, _)) = self.tx.seek(bound, true)? &&
            last[0] == self.table.id &&
            self.table.primary(&last) == self.table.primary(&raw) &&
            last >= raw
        {
            return Err(error("append duplicate is not ordered"));
        }
        self.upsert(key, &value)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;

    #[test]
    #[ignore = "DESTRUCTIVE: overwrites RETH_TRIEDB_TEST_DEVICE; requires RETH_TRIEDB_INITIALIZE=1"]
    fn native_raw_device_roundtrip() {
        assert_eq!(std::env::var("RETH_TRIEDB_INITIALIZE").as_deref(), Ok("1"));
        let path = std::env::var_os("RETH_TRIEDB_TEST_DEVICE")
            .expect("explicit disposable block device required");
        let path = Path::new(&path);
        assert!(std::fs::metadata(path).unwrap().file_type().is_block_device());
        let table = TableSpec::named("PlainAccountState").unwrap();
        let store = Store::open(path, true, false).unwrap();
        let tx = Transaction::new(store.clone(), EMPTY_VERSION, true);
        for i in 0..64u8 {
            let mut key = [0; 20];
            key[19] = i;
            tx.put(table, &key, &[i]).unwrap();
        }
        let version = tx.commit().unwrap();
        drop(tx);
        drop(store);
        let store = Store::open(path, false, true).unwrap();
        let tx = Transaction::new(store, version, false);
        assert_eq!(tx.entries(table).unwrap(), 64);
        for i in 0..64u8 {
            let mut key = [0; 20];
            key[19] = i;
            assert_eq!(tx.get(table, &key).unwrap(), Some(vec![i]));
        }
    }
}
