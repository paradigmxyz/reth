//! Mock database implementation for testing and development.
//!
//! Provides lightweight mock implementations of database traits backed by an
//! in-memory [`BTreeMap`] store.  Unlike the previous no-op stubs every call to
//! [`DbTxMut::put`] actually persists data that a subsequent [`DbTx::get`] or
//! cursor walk can read back.
//!
//! ## Storage layout
//!
//! All tables share a single `Arc<Mutex<BTreeMap<&'static str, TableMap>>>` keyed
//! by the table's [`Table::NAME`] constant.
//!
//! * **Regular tables** – stored key is the plain encoded key.
//! * **[`DupSort`] tables** – stored key is a composite:
//!
//! ```text
//! ┌──────────────────────────┬─────────────────┬──────────────────────────────┐
//! │ 8-byte BE main-key-len   │ main-key bytes  │ compressed-value bytes       │
//! └──────────────────────────┴─────────────────┴──────────────────────────────┘
//! ```
//!
//! This ordering mirrors the MDBX DupSort convention in which the leading bytes of
//! the compressed value act as the sub-key, giving the same sorted behaviour as a
//! real database.  [`seek_by_key_subkey`](DbDupCursorRO::seek_by_key_subkey) works
//! correctly for any DupSort table that follows this convention (which all tables in
//! reth do).

use crate::{
    common::{IterPairResult, PairResult, ValueOnlyResult},
    cursor::{
        DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, RangeWalker,
        ReverseWalker, Walker,
    },
    database::Database,
    database_metrics::DatabaseMetrics,
    table::{Compress, Decode, Decompress, DupSort, Encode, IntoVec, Table, TableImporter},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use core::ops::Bound;
use std::{
    collections::BTreeMap,
    marker::PhantomData,
    ops::RangeBounds,
    path::PathBuf,
    sync::{Arc, Mutex},
};

// ── DupSort composite-key helpers ─────────────────────────────────────────────

/// Byte-width of the main-key-length prefix in every DupSort composite key.
const KEY_LEN_PREFIX: usize = 8;

/// Build a DupSort composite storage key:
/// `[8-byte BE len(main)][main bytes][rest bytes]`
fn dup_composite(main_encoded: &[u8], rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(KEY_LEN_PREFIX + main_encoded.len() + rest.len());
    out.extend_from_slice(&(main_encoded.len() as u64).to_be_bytes());
    out.extend_from_slice(main_encoded);
    out.extend_from_slice(rest);
    out
}

/// Extract the main-key slice from a DupSort composite storage key, or `None` if
/// the bytes are too short to be valid.
fn main_key_of(composite: &[u8]) -> Option<&[u8]> {
    if composite.len() < KEY_LEN_PREFIX {
        return None;
    }
    let len = u64::from_be_bytes(composite[..KEY_LEN_PREFIX].try_into().ok()?) as usize;
    composite.get(KEY_LEN_PREFIX..KEY_LEN_PREFIX + len)
}

// ── shared storage type aliases ───────────────────────────────────────────────

/// Per-table row storage: stored-key → compressed-value bytes.
type TableMap = BTreeMap<Vec<u8>, Vec<u8>>;

/// The shared in-memory store: table name → [`TableMap`].
type SharedStorage = Arc<Mutex<BTreeMap<&'static str, TableMap>>>;

// ── DatabaseMock ──────────────────────────────────────────────────────────────

/// Mock database implementation for testing and development.
///
/// Backed by an in-memory `BTreeMap` shared across all transactions obtained from
/// this instance.  Every `put` / `delete` is immediately visible to subsequent
/// reads and cursor walks within any transaction.
#[derive(Clone, Debug, Default)]
pub struct DatabaseMock {
    /// Shared in-memory table storage.
    ///
    /// Outer key: [`Table::NAME`].  Inner key: encoded (or DupSort composite) key.
    /// Value: compressed value bytes.
    pub data: SharedStorage,
}

impl Database for DatabaseMock {
    type TX = TxMock;
    type TXMut = TxMock;

    fn tx(&self) -> Result<Self::TX, DatabaseError> {
        Ok(TxMock { db: Arc::clone(&self.data) })
    }

    fn tx_mut(&self) -> Result<Self::TXMut, DatabaseError> {
        Ok(TxMock { db: Arc::clone(&self.data) })
    }

    fn path(&self) -> PathBuf {
        PathBuf::default()
    }
}

impl DatabaseMetrics for DatabaseMock {}

// ── TxMock ────────────────────────────────────────────────────────────────────

/// Mock transaction that shares the database's [`SharedStorage`].
///
/// Both read and write operations go through the same `Arc<Mutex<…>>`, so a
/// `put` is immediately visible to a subsequent `get` within the same transaction
/// and to all other transactions.  `commit` and `abort` are intentional no-ops —
/// there is no snapshot isolation.
#[derive(Debug, Clone)]
pub struct TxMock {
    db: SharedStorage,
}

impl Default for TxMock {
    fn default() -> Self {
        Self { db: Arc::new(Mutex::new(BTreeMap::new())) }
    }
}

impl DbTx for TxMock {
    type Cursor<T: Table> = CursorMock<T>;
    type DupCursor<T: DupSort> = CursorMock<T>;

    /// Retrieves a value by key from the specified table.
    ///
    /// Returns `None` when the key does not exist.  For [`DupSort`] tables this
    /// returns the first duplicate value for the given main key.
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<T::Value>, DatabaseError> {
        let encoded = key.encode().into_vec();
        let guard = self.db.lock().unwrap();

        if T::DUPSORT {
            // For DupSort find the first entry whose main-key portion matches.
            let prefix = dup_composite(&encoded, &[]);
            guard
                .get(T::NAME)
                .and_then(|t| {
                    t.range(prefix..).next().and_then(|(k, v)| {
                        if main_key_of(k) == Some(&encoded) { Some(v) } else { None }
                    })
                })
                .map(|v| T::Value::decompress(v))
                .transpose()
        } else {
            guard
                .get(T::NAME)
                .and_then(|t| t.get(&encoded))
                .map(|v| T::Value::decompress(v))
                .transpose()
        }
    }

    /// Retrieves a value using a pre-encoded key reference.
    ///
    /// Avoids cloning a reference key (e.g. `Address`, `B256`) by operating
    /// directly on the encoded bytes.
fn get_by_encoded_key<T: Table>(
    &self,
    key: &<T::Key as Encode>::Encoded,
) -> Result<Option<T::Value>, DatabaseError> {
    let encoded: &[u8] = key.as_ref();
    let guard = self.db.lock().unwrap();

    if T::DUPSORT {
        // For DupSort find the first entry whose main-key portion matches `encoded`.
        let prefix = dup_composite(encoded, &[]);
        guard
            .get(T::NAME)
            .and_then(|t| {
                t.range(prefix..).next().and_then(|(k, v)| {
                    if main_key_of(k) == Some(encoded) { Some(v) } else { None }
                })
            })
            .map(|v| T::Value::decompress(v))
            .transpose()
    } else {
        guard
            .get(T::NAME)
            .and_then(|t| t.get(encoded))
            .map(|v| T::Value::decompress(v))
            .transpose()
    }
}

    /// Commits the transaction.
    ///
    /// **Mock behaviour**: always succeeds; storage mutations are applied eagerly
    /// so there is nothing to flush.
    fn commit(self) -> Result<(), DatabaseError> {
        Ok(())
    }

    /// Aborts the transaction.
    ///
    /// **Mock behaviour**: no-op.
    fn abort(self) {}

    /// Creates a read-only cursor for `T`.
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError> {
        Ok(CursorMock::new(Arc::clone(&self.db)))
    }

    /// Creates a read-only duplicate cursor for `T`.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError> {
        Ok(CursorMock::new(Arc::clone(&self.db)))
    }

    /// Returns the number of entries in the specified table.
    ///
    /// For [`DupSort`] tables every individual `(main-key, sub-key)` pair counts
    /// as one entry, matching real database semantics.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        Ok(self.db.lock().unwrap().get(T::NAME).map_or(0, |t| t.len()))
    }

    /// **Mock behaviour**: no-op.
    fn disable_long_read_transaction_safety(&mut self) {}
}

impl DbTxMut for TxMock {
    type CursorMut<T: Table> = CursorMock<T>;
    type DupCursorMut<T: DupSort> = CursorMock<T>;

    /// Inserts or updates a key-value pair in the specified table.
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let k = key.encode().into_vec();
        let v: Vec<u8> = value.compress().into();
        self.db.lock().unwrap().entry(T::NAME).or_default().insert(k, v);
        Ok(())
    }

    /// Deletes a key-value pair from the specified table.
    ///
    /// Returns `true` if the key existed.
    fn delete<T: Table>(
        &self,
        key: T::Key,
        _value: Option<T::Value>,
    ) -> Result<bool, DatabaseError> {
        let k = key.encode().into_vec();
        let removed =
            self.db.lock().unwrap().entry(T::NAME).or_default().remove(&k).is_some();
        Ok(removed)
    }

    /// Removes all entries from the specified table.
    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        self.db.lock().unwrap().entry(T::NAME).or_default().clear();
        Ok(())
    }

    /// Creates a read-write cursor for `T`.
    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError> {
        Ok(CursorMock::new(Arc::clone(&self.db)))
    }

    /// Creates a read-write duplicate cursor for `T`.
    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError> {
        Ok(CursorMock::new(Arc::clone(&self.db)))
    }
}

impl TableImporter for TxMock {}

// ── CursorMock ────────────────────────────────────────────────────────────────

/// Mock cursor backed by a point-in-time snapshot of one table.
///
/// The snapshot is a sorted `Vec<(stored_key, compressed_value)>` built from the
/// [`SharedStorage`] at construction time and refreshed after every write.
/// `position` is an index into this vec; `None` means "unpositioned".
///
/// ### DupSort
///
/// For tables where `T::DUPSORT == true` the stored keys follow the composite
/// layout described in the module docs.  All decode helpers strip the length
/// prefix and return only the main-key bytes to callers.
#[derive(Debug)]
pub struct CursorMock<T: Table> {
    db: SharedStorage,
    /// Sorted `(stored_key, compressed_value)` pairs – the cursor's view of the table.
    snapshot: Vec<(Vec<u8>, Vec<u8>)>,
    /// Current position within `snapshot`, or `None` when unpositioned.
    position: Option<usize>,
    _phantom: PhantomData<T>,
}

// ── construction & private helpers ────────────────────────────────────────────

impl<T: Table> CursorMock<T> {
    fn new(db: SharedStorage) -> Self {
        let snapshot = Self::load_snapshot(&db);
        Self { db, snapshot, position: None, _phantom: PhantomData }
    }

    fn load_snapshot(db: &SharedStorage) -> Vec<(Vec<u8>, Vec<u8>)> {
        db.lock()
            .unwrap()
            .get(T::NAME)
            .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// Reload the snapshot from shared storage and reposition the cursor at
    /// `written_key` (the entry that was just inserted or updated).
    fn refresh(&mut self, written_key: &[u8]) {
        self.snapshot = Self::load_snapshot(&self.db);
        self.position = self.snapshot.iter().position(|(k, _)| k.as_slice() == written_key);
    }

    // ── decode ──────────────────────────────────────────────────────────────

    fn decode_key_at(&self, idx: usize) -> Result<T::Key, DatabaseError> {
        let raw = &self.snapshot[idx].0;
        let bytes = if T::DUPSORT {
            main_key_of(raw).ok_or(DatabaseError::Decode)?
        } else {
            raw.as_slice()
        };
        T::Key::decode(bytes)
    }

    fn decode_value_at(&self, idx: usize) -> Result<T::Value, DatabaseError> {
        T::Value::decompress(&self.snapshot[idx].1)
    }

    fn pair_at(&self, idx: usize) -> PairResult<T> {
        Ok(Some((self.decode_key_at(idx)?, self.decode_value_at(idx)?)))
    }

    // ── navigation ──────────────────────────────────────────────────────────

    /// First index `i` where `snapshot[i].0 >= needle` (binary search).
    fn lower_bound(&self, needle: &[u8]) -> usize {
        self.snapshot.partition_point(|(k, _)| k.as_slice() < needle)
    }

    /// Set `position = idx` and return the decoded pair there.
    fn go(&mut self, idx: usize) -> PairResult<T> {
        self.position = Some(idx);
        self.pair_at(idx)
    }

    // ── DupSort position helpers ─────────────────────────────────────────────

    /// Raw main-key bytes at `idx` in the snapshot (only meaningful for DupSort).
    fn main_key_at(&self, idx: usize) -> Option<&[u8]> {
        main_key_of(&self.snapshot[idx].0)
    }

    /// Owned copy of the main-key bytes at the current position.
    fn current_main_key(&self) -> Option<Vec<u8>> {
        self.position.and_then(|i| self.main_key_at(i)).map(<[u8]>::to_vec)
    }
}

// ── DbCursorRO<T> ─────────────────────────────────────────────────────────────

impl<T: Table> DbCursorRO<T> for CursorMock<T> {
    fn first(&mut self) -> PairResult<T> {
        if self.snapshot.is_empty() {
            self.position = None;
            return Ok(None);
        }
        self.go(0)
    }

    fn seek_exact(&mut self, key: T::Key) -> PairResult<T> {
        let enc = key.encode().into_vec();
        let needle = if T::DUPSORT { dup_composite(&enc, &[]) } else { enc.clone() };
        let idx = self.lower_bound(&needle);

        if idx >= self.snapshot.len() {
            self.position = None;
            return Ok(None);
        }

        // Regular table: exact key match.
        // DupSort: first composite entry whose main-key portion equals `enc`.
        let matches = if T::DUPSORT {
            self.main_key_at(idx) == Some(&enc)
        } else {
            self.snapshot[idx].0 == enc
        };

        if matches { self.go(idx) } else { self.position = None; Ok(None) }
    }

    fn seek(&mut self, key: T::Key) -> PairResult<T> {
        let enc = key.encode().into_vec();
        let needle = if T::DUPSORT { dup_composite(&enc, &[]) } else { enc };
        let idx = self.lower_bound(&needle);
        if idx >= self.snapshot.len() {
            self.position = None;
            return Ok(None);
        }
        self.go(idx)
    }

    fn next(&mut self) -> PairResult<T> {
        let next = match self.position {
            Some(p) => p + 1,
            None => return Ok(None),
        };
        if next >= self.snapshot.len() {
            self.position = None;
            return Ok(None);
        }
        self.go(next)
    }

    fn prev(&mut self) -> PairResult<T> {
        match self.position {
            None | Some(0) => {
                self.position = None;
                Ok(None)
            }
            Some(p) => self.go(p - 1),
        }
    }

    fn last(&mut self) -> PairResult<T> {
        let n = self.snapshot.len();
        if n == 0 {
            self.position = None;
            return Ok(None);
        }
        self.go(n - 1)
    }

    fn current(&mut self) -> PairResult<T> {
        match self.position {
            Some(i) => self.pair_at(i),
            None => Ok(None),
        }
    }

    fn walk(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<Walker<'_, T, Self>, DatabaseError> {
        let start: IterPairResult<T> = match start_key {
            Some(k) => self.seek(k).transpose(),
            None => self.first().transpose(),
        };
        Ok(Walker::new(self, start))
    }

    fn walk_range(
        &mut self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<RangeWalker<'_, T, Self>, DatabaseError> {
        let start_key: Option<T::Key> = match range.start_bound() {
            Bound::Included(k) | Bound::Excluded(k) => Some(k.clone()),
            Bound::Unbounded => None,
        };
        let end_key: Bound<T::Key> = match range.end_bound() {
            Bound::Included(k) => Bound::Included(k.clone()),
            Bound::Excluded(k) => Bound::Excluded(k.clone()),
            Bound::Unbounded => Bound::Unbounded,
        };
        let start: IterPairResult<T> = match start_key {
            Some(k) => self.seek(k).transpose(),
            None => self.first().transpose(),
        };
        Ok(RangeWalker::new(self, start, end_key))
    }

    fn walk_back(
        &mut self,
        start_key: Option<T::Key>,
    ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError> {
        let start: IterPairResult<T> = match start_key {
            Some(k) => self.seek(k).transpose(),
            None => self.last().transpose(),
        };
        Ok(ReverseWalker::new(self, start))
    }
}

// ── DbDupCursorRO<T> ──────────────────────────────────────────────────────────

impl<T: DupSort> DbDupCursorRO<T> for CursorMock<T> {
    /// Moves to the next duplicate entry (same main key).
    /// Returns `None` when the next entry belongs to a different main key.
    fn next_dup(&mut self) -> PairResult<T> {
        let Some(pos) = self.position else { return Ok(None) };
        let next = pos + 1;
        if next >= self.snapshot.len() {
            self.position = None;
            return Ok(None);
        }
        if self.main_key_at(pos) != self.main_key_at(next) {
            self.position = None;
            return Ok(None);
        }
        self.go(next)
    }

    /// Moves to the previous duplicate entry (same main key).
    fn prev_dup(&mut self) -> PairResult<T> {
        let Some(pos) = self.position else { return Ok(None) };
        if pos == 0 {
            self.position = None;
            return Ok(None);
        }
        if self.main_key_at(pos) != self.main_key_at(pos - 1) {
            self.position = None;
            return Ok(None);
        }
        self.go(pos - 1)
    }

    /// Positions at the last duplicate of the current main key and returns its value.
    fn last_dup(&mut self) -> ValueOnlyResult<T> {
        let Some(pos) = self.position else { return Ok(None) };
        let main = self.main_key_at(pos).map(<[u8]>::to_vec);
        let mut last = pos;
        for i in (pos + 1)..self.snapshot.len() {
            if self.main_key_at(i).map(<[u8]>::to_vec) == main {
                last = i;
            } else {
                break;
            }
        }
        self.position = Some(last);
        self.decode_value_at(last).map(Some)
    }

    /// Advances to the first entry of the next main key, skipping remaining
    /// duplicates for the current key.
    fn next_no_dup(&mut self) -> PairResult<T> {
        let current_main = self.current_main_key();
        let start = self.position.map(|p| p + 1).unwrap_or(0);
        for i in start..self.snapshot.len() {
            if self.main_key_at(i).map(<[u8]>::to_vec) != current_main {
                return self.go(i);
            }
        }
        self.position = None;
        Ok(None)
    }

    /// Returns the value of the next duplicate without changing the cursor
    /// semantics otherwise.
    fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
        Ok(self.next_dup()?.map(|(_, v)| v))
    }

    /// Seeks to the first entry with `main key == key` and `sub-key >= subkey`.
    ///
    /// Relies on the MDBX convention that `encode(sub_key)` occupies the leading
    /// bytes of the compressed value, which all reth DupSort tables follow.
    fn seek_by_key_subkey(
        &mut self,
        key: T::Key,
        subkey: T::SubKey,
    ) -> ValueOnlyResult<T> {
        let enc_main = key.encode().into_vec();
        let enc_sub = subkey.encode().into_vec();
        // The stored composite key is [len][main][compressed-value-bytes], and
        // compressed-value starts with encode(sub_key), so:
        //   needle = [len][main][encode(sub_key)]
        // is a prefix of the first stored entry whose sub-key >= `subkey`.
        let needle = dup_composite(&enc_main, &enc_sub);
        let idx = self.lower_bound(&needle);

        if idx >= self.snapshot.len() {
            self.position = None;
            return Ok(None);
        }
        // Accept only if we're still in the same main-key group.
        if self.main_key_at(idx) == Some(&enc_main) {
            self.position = Some(idx);
            self.decode_value_at(idx).map(Some)
        } else {
            self.position = None;
            Ok(None)
        }
    }

    fn walk_dup(
        &mut self,
        key: Option<T::Key>,
        subkey: Option<T::SubKey>,
    ) -> Result<DupWalker<'_, T, Self>, DatabaseError> {
        let start: IterPairResult<T> = match (key, subkey) {
            (Some(k), Some(sk)) => {
                // seek_by_key_subkey positions the cursor; read current pair.
                self.seek_by_key_subkey(k, sk)?;
                self.current().transpose()
            }
            (Some(k), None) => self.seek_exact(k).transpose(),
            // (None, Some(_)) and (None, None) both start from the first entry.
            (None, _) => self.first().transpose(),
        };
        Ok(DupWalker { cursor: self, start })
    }
}

// ── DbCursorRW<T> ─────────────────────────────────────────────────────────────

impl<T: Table> DbCursorRW<T> for CursorMock<T> {
    /// Inserts or updates the entry at `key`.
    ///
    /// For [`DupSort`] tables the composite key includes the compressed value so
    /// that the same `(key, value)` pair is idempotent while distinct sub-key
    /// values produce separate rows.
    fn upsert(
        &mut self,
        key: T::Key,
        value: &T::Value,
    ) -> Result<(), DatabaseError> {
        let mut buf = <<T as crate::table::Table>::Value as crate::table::Compress>::Compressed::default();;
        value.compress_to_buf(&mut buf);
        let compressed: Vec<u8> = buf.into();

        let stored_key = if T::DUPSORT {
            dup_composite(&key.encode().into_vec(), &compressed)
        } else {
            key.encode().into_vec()
        };

        self.db.lock().unwrap().entry(T::NAME).or_default().insert(stored_key.clone(), compressed);
        self.refresh(&stored_key);
        Ok(())
    }

    /// Inserts an entry at `key`.
    ///
    /// **Mock behaviour**: the "no overwrite" contract is not enforced; treated
    /// as [`upsert`](Self::upsert).
    fn insert(
        &mut self,
        key: T::Key,
        value: &T::Value,
    ) -> Result<(), DatabaseError> {
        self.upsert(key, value)
    }

    /// Appends a key-value pair.
    ///
    /// Efficient for pre-sorted data in real databases; in the mock this is
    /// equivalent to [`upsert`](Self::upsert).
    fn append(
        &mut self,
        key: T::Key,
        value: &T::Value,
    ) -> Result<(), DatabaseError> {
        self.upsert(key, value)
    }

    /// Deletes the entry at the current cursor position.
    ///
    /// After deletion the cursor moves to the successor entry (the entry that now
    /// occupies the same index), matching MDBX semantics.
    fn delete_current(&mut self) -> Result<(), DatabaseError> {
        let Some(idx) = self.position else { return Ok(()) };
        let stored_key = self.snapshot[idx].0.clone();
        self.db.lock().unwrap().entry(T::NAME).or_default().remove(&stored_key);

        self.snapshot = Self::load_snapshot(&self.db);
        // If entries remain at `idx` the cursor is now on the logical successor.
        self.position = if self.snapshot.is_empty() {
            None
        } else if idx < self.snapshot.len() {
            Some(idx)
        } else {
            None // was the last entry
        };
        Ok(())
    }
}

// ── DbDupCursorRW<T> ──────────────────────────────────────────────────────────

impl<T: DupSort> DbDupCursorRW<T> for CursorMock<T> {
    /// Deletes all duplicate entries for the current main key.
    ///
    /// After deletion the cursor moves to the first entry of the next main key,
    /// or becomes unpositioned if no further keys exist.
    fn delete_current_duplicates(&mut self) -> Result<(), DatabaseError> {
        let Some(pos) = self.position else { return Ok(()) };
        let main = match self.main_key_at(pos) {
            Some(k) => k.to_vec(),
            None => return Ok(()),
        };
        let prefix = dup_composite(&main, &[]);
        {
            let mut guard = self.db.lock().unwrap();
            if let Some(table) = guard.get_mut(T::NAME) {
                table.retain(|k, _| !k.starts_with(&prefix));
            }
        }
        self.snapshot = Self::load_snapshot(&self.db);
        // Position at the first entry of the next key group.
        let next_idx = self.lower_bound(&prefix);
        self.position = if next_idx < self.snapshot.len() { Some(next_idx) } else { None };
        Ok(())
    }

    /// Appends a duplicate value for `key`.
    ///
    /// Stores the entry under a composite key so it co-exists with other sub-key
    /// values for the same main key.
    fn append_dup(
        &mut self,
        key: T::Key,
        value: T::Value,
    ) -> Result<(), DatabaseError> {
        let mut buf = <<T as crate::table::Table>::Value as crate::table::Compress>::Compressed::default();
        value.compress_to_buf(&mut buf);
        let compressed: Vec<u8> = buf.into();
        let stored_key = dup_composite(&key.encode().into_vec(), &compressed);
        self.db.lock().unwrap().entry(T::NAME).or_default().insert(stored_key.clone(), compressed);
        self.refresh(&stored_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::Database,
        tables::{CanonicalHeaders, PlainAccountState},
        transaction::{DbTx, DbTxMut},
    };
    use alloy_primitives::{Address, B256};
    use reth_primitives_traits::Account;

    #[test]
    fn test_put_and_get_roundtrip() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        let block_num: u64 = 42;
        let hash = B256::repeat_byte(0xab);

        tx.put::<CanonicalHeaders>(block_num, hash).unwrap();
        assert_eq!(tx.get::<CanonicalHeaders>(block_num).unwrap(), Some(hash));
        // Unknown key returns None.
        assert_eq!(tx.get::<CanonicalHeaders>(99).unwrap(), None);
    }

    #[test]
    fn test_delete_removes_entry() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        let addr = Address::repeat_byte(1);
        tx.put::<PlainAccountState>(addr, Account::default()).unwrap();
        assert!(tx.get::<PlainAccountState>(addr).unwrap().is_some());

        assert!(tx.delete::<PlainAccountState>(addr, None).unwrap());
        assert!(tx.get::<PlainAccountState>(addr).unwrap().is_none());
        // Deleting a non-existent key returns false.
        assert!(!tx.delete::<PlainAccountState>(addr, None).unwrap());
    }

    #[test]
    fn test_cursor_walk_iterates_in_order() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        for i in 0u64..5 {
            tx.put::<CanonicalHeaders>(i, B256::repeat_byte(i as u8)).unwrap();
        }

        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let entries: Vec<_> =
            cursor.walk(None).unwrap().collect::<Result<_, _>>().unwrap();

        assert_eq!(entries.len(), 5);
        for (i, (k, v)) in entries.iter().enumerate() {
            assert_eq!(*k, i as u64);
            assert_eq!(*v, B256::repeat_byte(i as u8));
        }
    }

    #[test]
    fn test_cursor_seek_exact() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        tx.put::<CanonicalHeaders>(10u64, B256::repeat_byte(0x10)).unwrap();
        tx.put::<CanonicalHeaders>(20u64, B256::repeat_byte(0x20)).unwrap();

        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        assert_eq!(
            cursor.seek_exact(10u64).unwrap(),
            Some((10u64, B256::repeat_byte(0x10)))
        );
        // Key 15 does not exist.
        assert_eq!(cursor.seek_exact(15u64).unwrap(), None);
    }

    #[test]
    fn test_cursor_seek_finds_gte() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        tx.put::<CanonicalHeaders>(10u64, B256::repeat_byte(0x10)).unwrap();
        tx.put::<CanonicalHeaders>(20u64, B256::repeat_byte(0x20)).unwrap();

        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        // Seek to key 15 → should land on 20.
        assert_eq!(
            cursor.seek(15u64).unwrap(),
            Some((20u64, B256::repeat_byte(0x20)))
        );
    }

    #[test]
    fn test_cursor_walk_range() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        for i in 0u64..10 {
            tx.put::<CanonicalHeaders>(i, B256::repeat_byte(i as u8)).unwrap();
        }

        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let entries: Vec<_> = cursor
            .walk_range(3u64..=6u64)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert_eq!(entries.len(), 4);
        assert_eq!(entries.first().unwrap().0, 3u64);
        assert_eq!(entries.last().unwrap().0, 6u64);
    }

    #[test]
    fn test_cursor_walk_back() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        for i in 0u64..4 {
            tx.put::<CanonicalHeaders>(i, B256::repeat_byte(i as u8)).unwrap();
        }

        let mut cursor = tx.cursor_read::<CanonicalHeaders>().unwrap();
        let rev: Vec<_> =
            cursor.walk_back(None).unwrap().collect::<Result<_, _>>().unwrap();

        assert_eq!(rev.len(), 4);
        assert_eq!(rev[0].0, 3u64); // starts from last
        assert_eq!(rev[3].0, 0u64);
    }

    #[test]
    fn test_entries_count() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        assert_eq!(tx.entries::<CanonicalHeaders>().unwrap(), 0);
        tx.put::<CanonicalHeaders>(1u64, B256::ZERO).unwrap();
        tx.put::<CanonicalHeaders>(2u64, B256::ZERO).unwrap();
        assert_eq!(tx.entries::<CanonicalHeaders>().unwrap(), 2);
    }

    #[test]
    fn test_clear_removes_all_entries() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        for i in 0u64..3 {
            tx.put::<CanonicalHeaders>(i, B256::ZERO).unwrap();
        }
        tx.clear::<CanonicalHeaders>().unwrap();
        assert_eq!(tx.entries::<CanonicalHeaders>().unwrap(), 0);
    }

    #[test]
    fn test_cursor_delete_current_positions_on_successor() {
        let db = DatabaseMock::default();
        let tx = db.tx_mut().unwrap();

        for i in 0u64..3 {
            tx.put::<CanonicalHeaders>(i, B256::repeat_byte(i as u8)).unwrap();
        }

        let mut cursor = tx.cursor_write::<CanonicalHeaders>().unwrap();
        cursor.seek_exact(1u64).unwrap();
        cursor.delete_current().unwrap();

        // After deletion of key 1, cursor should be on the successor (key 2).
        assert_eq!(cursor.current().unwrap().map(|(k, _)| k), Some(2u64));

        // Only keys 0 and 2 remain.
        let remaining: Vec<_> = tx
            .cursor_read::<CanonicalHeaders>()
            .unwrap()
            .walk(None)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].0, 0u64);
        assert_eq!(remaining[1].0, 2u64);
    }

    #[test]
    fn test_transactions_share_storage() {
        let db = DatabaseMock::default();

        let tx1 = db.tx_mut().unwrap();
        tx1.put::<CanonicalHeaders>(7u64, B256::repeat_byte(7)).unwrap();

        // A second transaction on the same db sees the write immediately.
        let tx2 = db.tx().unwrap();
        assert_eq!(
            tx2.get::<CanonicalHeaders>(7u64).unwrap(),
            Some(B256::repeat_byte(7))
        );
    }
}