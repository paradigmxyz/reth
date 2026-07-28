use alloy_primitives::B256;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    table::{DupSort, DupSortSubKey, Key, Table, Value},
    tables::{self, PackedAccountsTrie, PackedStoragesTrie},
    transaction::DbTx,
    DatabaseError,
};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::StorageTrieUpdatesSorted,
    BranchNodeCompact, Nibbles, PackedStorageTrieEntry, PackedStoredNibbles,
    PackedStoredNibblesSubKey, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey,
};
use std::marker::PhantomData;

/// Trait abstracting nibble encoding for trie keys.
///
/// Allows the same cursor implementation to work with both legacy (65-byte) and
/// packed (33-byte) nibble encodings. The underlying cursor types are monomorphized per
/// adapter, while [`DatabaseTrieCursorFactory`] selects the encoding at runtime.
pub trait TrieKeyAdapter: Clone + Send + Sync + 'static {
    /// The key type for account trie lookups (e.g., `StoredNibbles` or `PackedStoredNibbles`).
    type AccountKey: Key + From<Nibbles> + Clone;

    /// The subkey type for storage trie `DupSort` lookups
    /// (e.g., `StoredNibblesSubKey` or `PackedStoredNibblesSubKey`).
    type StorageSubKey: Key + DupSortSubKey + From<Nibbles> + Clone + PartialEq;

    /// The storage trie entry type that pairs a subkey with a `BranchNodeCompact`.
    type StorageValue: Value + StorageTrieEntryLike<SubKey = Self::StorageSubKey>;

    /// Convert an account key back to `Nibbles`.
    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles;

    /// Convert a storage subkey back to `Nibbles`.
    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles;
}

/// Trait for storage trie entry types that carry a subkey and node.
///
/// Needed because [`StorageTrieEntry`] and [`PackedStorageTrieEntry`] are separate structs
/// with different field types, but `DatabaseStorageTrieCursor` must access `.nibbles()` and
/// `.node()` generically through `A::StorageValue`.
pub trait StorageTrieEntryLike: Sized {
    /// The subkey type.
    type SubKey: Clone;

    /// Returns a reference to the nibbles subkey.
    fn nibbles(&self) -> &Self::SubKey;

    /// Returns a reference to the branch node.
    fn node(&self) -> &BranchNodeCompact;

    /// Decompose this value into owned parts.
    fn into_parts(self) -> (Self::SubKey, BranchNodeCompact);

    /// Construct a new entry from a subkey and node.
    fn new(nibbles: Self::SubKey, node: BranchNodeCompact) -> Self;
}

impl StorageTrieEntryLike for StorageTrieEntry {
    type SubKey = StoredNibblesSubKey;

    fn nibbles(&self) -> &Self::SubKey {
        &self.nibbles
    }

    fn node(&self) -> &BranchNodeCompact {
        &self.node
    }

    fn into_parts(self) -> (Self::SubKey, BranchNodeCompact) {
        (self.nibbles, self.node)
    }

    fn new(nibbles: Self::SubKey, node: BranchNodeCompact) -> Self {
        Self { nibbles, node }
    }
}

/// Legacy (v1) nibble encoding: 1 nibble per byte, 65-byte subkeys.
#[derive(Debug, Clone)]
pub struct LegacyKeyAdapter;

impl TrieKeyAdapter for LegacyKeyAdapter {
    type AccountKey = StoredNibbles;
    type StorageSubKey = StoredNibblesSubKey;
    type StorageValue = StorageTrieEntry;

    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles {
        key.0
    }

    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles {
        subkey.0
    }
}

impl StorageTrieEntryLike for PackedStorageTrieEntry {
    type SubKey = PackedStoredNibblesSubKey;

    fn nibbles(&self) -> &Self::SubKey {
        &self.nibbles
    }

    fn node(&self) -> &BranchNodeCompact {
        &self.node
    }

    fn into_parts(self) -> (Self::SubKey, BranchNodeCompact) {
        (self.nibbles, self.node)
    }

    fn new(nibbles: Self::SubKey, node: BranchNodeCompact) -> Self {
        Self { nibbles, node }
    }
}

/// Packed (v2) nibble encoding: 2 nibbles per byte, 33-byte subkeys.
#[derive(Debug, Clone)]
pub struct PackedKeyAdapter;

impl TrieKeyAdapter for PackedKeyAdapter {
    type AccountKey = PackedStoredNibbles;
    type StorageSubKey = PackedStoredNibblesSubKey;
    type StorageValue = PackedStorageTrieEntry;

    fn account_key_to_nibbles(key: &Self::AccountKey) -> Nibbles {
        key.0
    }

    fn subkey_to_nibbles(subkey: &Self::StorageSubKey) -> Nibbles {
        subkey.0
    }
}

/// Helper trait to map a [`TrieKeyAdapter`] to the correct table types.
///
/// This indirection is needed because the `tables!` macro generates non-generic
/// table types, so we use separate "view" types for packed encoding that share
/// the same MDBX table name.
pub trait TrieTableAdapter: TrieKeyAdapter {
    /// The account trie table type.
    type AccountTrieTable: Table<Key = Self::AccountKey, Value = BranchNodeCompact>;
    /// The storage trie table type.
    type StorageTrieTable: Table<Key = B256, Value = Self::StorageValue>
        + DupSort<SubKey = Self::StorageSubKey>;
}

impl TrieTableAdapter for LegacyKeyAdapter {
    type AccountTrieTable = tables::AccountsTrie;
    type StorageTrieTable = tables::StoragesTrie;
}

impl TrieTableAdapter for PackedKeyAdapter {
    type AccountTrieTable = PackedAccountsTrie;
    type StorageTrieTable = PackedStoragesTrie;
}

/// Wrapper struct for database transaction implementing trie cursor factory trait.
#[derive(Debug, Clone)]
pub struct DatabaseTrieCursorFactory<T, A: TrieKeyAdapter> {
    tx: T,
    _adapter: PhantomData<A>,
}

impl<T, A: TrieKeyAdapter> DatabaseTrieCursorFactory<T, A> {
    /// Create new [`DatabaseTrieCursorFactory`].
    pub const fn new(tx: T) -> Self {
        Self { tx, _adapter: PhantomData }
    }
}

impl<TX, A> TrieCursorFactory for DatabaseTrieCursorFactory<&TX, A>
where
    TX: DbTx,
    A: TrieTableAdapter,
{
    type AccountTrieCursor<'a>
        = DatabaseAccountTrieCursor<<TX as DbTx>::Cursor<A::AccountTrieTable>, A>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = DatabaseStorageTrieCursor<<TX as DbTx>::DupCursor<A::StorageTrieTable>, A>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseAccountTrieCursor::new(self.tx.cursor_read::<A::AccountTrieTable>()?))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseStorageTrieCursor::new(
            self.tx.cursor_dup_read::<A::StorageTrieTable>()?,
            hashed_address,
        ))
    }
}

/// A cursor over the account trie.
#[derive(Debug)]
pub struct DatabaseAccountTrieCursor<C, A: TrieKeyAdapter> {
    cursor: C,
    current_key: Option<Nibbles>,
    _adapter: PhantomData<A>,
}

impl<C, A: TrieKeyAdapter> DatabaseAccountTrieCursor<C, A> {
    /// Create a new account trie cursor.
    pub const fn new(cursor: C) -> Self {
        Self { cursor, current_key: None, _adapter: PhantomData }
    }
}

impl<C, A> DatabaseAccountTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::AccountTrieTable>,
{
    /// Advances from the current key to the first key greater than or equal to `target`.
    fn advance_to(&mut self, target: Nibbles) -> Result<Option<Nibbles>, DatabaseError> {
        let Some(mut current_key) = self.current_key else { return Ok(None) };

        while current_key < target {
            let next_key = match self.cursor.next_key() {
                Ok(next_key) => next_key,
                Err(error) => {
                    self.current_key = None;
                    return Err(error)
                }
            };
            let Some(next_key) = next_key else {
                self.current_key = None;
                return Ok(None)
            };
            current_key = A::account_key_to_nibbles(&next_key);
            self.current_key = Some(current_key);
        }

        Ok(Some(current_key))
    }

    /// Returns and tracks the entry at the underlying cursor's current position.
    fn current_entry(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = match self.cursor.current() {
            Ok(entry) => entry.map(|(key, value)| (A::account_key_to_nibbles(&key), value)),
            Err(error) => {
                self.current_key = None;
                return Err(error)
            }
        };
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }
}

impl<C, A> TrieCursor for DatabaseAccountTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::AccountTrieTable> + Send,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if self.current_key.is_some_and(|current_key| key.starts_with(&current_key)) {
            return if self.advance_to(key)? == Some(key) { self.current_entry() } else { Ok(None) }
        }

        self.current_key = None;
        let entry = self
            .cursor
            .seek_exact(A::AccountKey::from(key))?
            .map(|(key, value)| (A::account_key_to_nibbles(&key), value));
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if self.current_key.is_some_and(|current_key| key.starts_with(&current_key)) {
            return if self.advance_to(key)?.is_some() { self.current_entry() } else { Ok(None) }
        }

        self.current_key = None;
        let entry = self
            .cursor
            .seek(A::AccountKey::from(key))?
            .map(|(key, value)| (A::account_key_to_nibbles(&key), value));
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.current_key = None;
        let entry =
            self.cursor.next()?.map(|(key, value)| (A::account_key_to_nibbles(&key), value));
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.current_entry()?.map(|(key, _)| key))
    }

    fn reset(&mut self) {
        self.current_key = None;
    }
}

/// A cursor over the storage tries stored in the database.
#[derive(Debug)]
pub struct DatabaseStorageTrieCursor<C, A: TrieKeyAdapter> {
    /// The underlying cursor.
    cursor: C,
    /// Hashed address used for cursor positioning.
    hashed_address: B256,
    /// The subkey at the underlying cursor's current position.
    current_key: Option<Nibbles>,
    _adapter: PhantomData<A>,
}

impl<C, A: TrieKeyAdapter> DatabaseStorageTrieCursor<C, A> {
    /// Create a new storage trie cursor.
    pub const fn new(cursor: C, hashed_address: B256) -> Self {
        Self { cursor, hashed_address, current_key: None, _adapter: PhantomData }
    }

    /// Consumes the trie cursor and returns the underlying database cursor.
    pub fn into_inner(self) -> C {
        self.cursor
    }
}

impl<C, A> DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable> + DbDupCursorRO<A::StorageTrieTable>,
{
    /// Advances from the current subkey to the first subkey greater than or equal to `target`.
    fn advance_to(&mut self, target: Nibbles) -> Result<Option<Nibbles>, DatabaseError> {
        let Some(mut current_key) = self.current_key else { return Ok(None) };

        while current_key < target {
            let next_key = match self.cursor.next_dup_key() {
                Ok(next_key) => next_key,
                Err(error) => {
                    self.current_key = None;
                    return Err(error)
                }
            };
            let Some(next_key) = next_key else {
                self.current_key = None;
                return Ok(None)
            };
            current_key = A::subkey_to_nibbles(&next_key);
            self.current_key = Some(current_key);
        }

        Ok(Some(current_key))
    }

    /// Returns and tracks the entry at the underlying cursor's current position.
    fn current_entry(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = match self.cursor.current() {
            Ok(Some((key, value))) if key == self.hashed_address => {
                let (subkey, node) = value.into_parts();
                Some((A::subkey_to_nibbles(&subkey), node))
            }
            Ok(_) => None,
            Err(error) => {
                self.current_key = None;
                return Err(error)
            }
        };
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }
}

impl<C, A> DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable>
        + DbCursorRW<A::StorageTrieTable>
        + DbDupCursorRO<A::StorageTrieTable>
        + DbDupCursorRW<A::StorageTrieTable>,
{
    /// Writes storage updates that are already sorted
    pub fn write_storage_trie_updates_sorted(
        &mut self,
        updates: &StorageTrieUpdatesSorted,
    ) -> Result<usize, DatabaseError> {
        self.current_key = None;

        // The storage trie for this account has to be deleted.
        if updates.is_deleted() && self.cursor.seek_exact(self.hashed_address)?.is_some() {
            self.cursor.delete_current_duplicates()?;
        }

        let mut num_entries = 0;
        for (nibbles, maybe_updated) in updates.storage_nodes.iter().filter(|(n, _)| !n.is_empty())
        {
            num_entries += 1;
            let nibbles = A::StorageSubKey::from(*nibbles);
            // Delete the old entry if it exists.
            if self
                .cursor
                .seek_by_key_subkey(self.hashed_address, nibbles.clone())?
                .as_ref()
                .is_some_and(|e| *e.nibbles() == nibbles)
            {
                self.cursor.delete_current()?;
            }

            // There is an updated version of this node, insert new entry.
            if let Some(node) = maybe_updated {
                self.cursor
                    .upsert(self.hashed_address, &A::StorageValue::new(nibbles, node.clone()))?;
            }
        }

        Ok(num_entries)
    }
}

impl<C, A> TrieCursor for DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable> + DbDupCursorRO<A::StorageTrieTable> + Send,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if self.current_key.is_some_and(|current_key| key.starts_with(&current_key)) {
            return if self.advance_to(key)? == Some(key) { self.current_entry() } else { Ok(None) }
        }

        self.current_key = None;
        let entry = self
            .cursor
            .seek_by_key_subkey(self.hashed_address, A::StorageSubKey::from(key))?
            .map(|value| {
                let (subkey, node) = value.into_parts();
                (A::subkey_to_nibbles(&subkey), node)
            });
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry.filter(|(found_key, _)| *found_key == key))
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if self.current_key.is_some_and(|current_key| key.starts_with(&current_key)) {
            return if self.advance_to(key)?.is_some() { self.current_entry() } else { Ok(None) }
        }

        self.current_key = None;
        let entry = self
            .cursor
            .seek_by_key_subkey(self.hashed_address, A::StorageSubKey::from(key))?
            .map(|value| {
                let (subkey, node) = value.into_parts();
                (A::subkey_to_nibbles(&subkey), node)
            });
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.current_key = None;
        let entry = self.cursor.next_dup()?.map(|(_, value)| {
            let (subkey, node) = value.into_parts();
            (A::subkey_to_nibbles(&subkey), node)
        });
        self.current_key = entry.as_ref().map(|(key, _)| *key);
        Ok(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.current_entry()?.map(|(key, _)| key))
    }

    fn reset(&mut self) {
        self.current_key = None;
    }
}

impl<C, A> TrieStorageCursor for DatabaseStorageTrieCursor<C, A>
where
    A: TrieTableAdapter,
    C: DbCursorRO<A::StorageTrieTable> + DbDupCursorRO<A::StorageTrieTable> + Send,
{
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.hashed_address = hashed_address;
        self.current_key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex_literal::hex;
    use reth_db_api::{
        common::{KeyOnlyResult, PairResult, SubKeyOnlyResult, ValueOnlyResult},
        cursor::{DbCursorRW, DupWalker, RangeWalker, ReverseWalker, Walker},
        transaction::DbTxMut,
    };
    use reth_provider::test_utils::create_test_provider_factory;
    use std::ops::{Bound, RangeBounds};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct CursorCalls {
        seek_exact: usize,
        seek: usize,
        next: usize,
        next_key: usize,
        next_dup: usize,
        next_dup_key: usize,
        seek_by_key_subkey: usize,
        current: usize,
    }

    struct RecordingCursor<C> {
        inner: C,
        calls: CursorCalls,
    }

    impl<C> RecordingCursor<C> {
        const fn new(inner: C) -> Self {
            Self {
                inner,
                calls: CursorCalls {
                    seek_exact: 0,
                    seek: 0,
                    next: 0,
                    next_key: 0,
                    next_dup: 0,
                    next_dup_key: 0,
                    seek_by_key_subkey: 0,
                    current: 0,
                },
            }
        }
    }

    impl<T, C> DbDupCursorRO<T> for RecordingCursor<C>
    where
        T: DupSort,
        C: DbDupCursorRO<T>,
    {
        fn prev_dup(&mut self) -> PairResult<T> {
            self.inner.prev_dup()
        }

        fn next_dup(&mut self) -> PairResult<T> {
            self.calls.next_dup += 1;
            self.inner.next_dup()
        }

        fn next_dup_key(&mut self) -> SubKeyOnlyResult<T>
        where
            T::SubKey: DupSortSubKey,
        {
            self.calls.next_dup_key += 1;
            self.inner.next_dup_key()
        }

        fn last_dup(&mut self) -> ValueOnlyResult<T> {
            self.inner.last_dup()
        }

        fn next_no_dup(&mut self) -> PairResult<T> {
            self.inner.next_no_dup()
        }

        fn next_dup_val(&mut self) -> ValueOnlyResult<T> {
            self.inner.next_dup_val()
        }

        fn seek_by_key_subkey(&mut self, key: T::Key, subkey: T::SubKey) -> ValueOnlyResult<T> {
            self.calls.seek_by_key_subkey += 1;
            self.inner.seek_by_key_subkey(key, subkey)
        }

        fn seek_by_key_subkey_key(&mut self, key: T::Key, subkey: T::SubKey) -> SubKeyOnlyResult<T>
        where
            T::SubKey: DupSortSubKey,
        {
            self.inner.seek_by_key_subkey_key(key, subkey)
        }

        fn walk_dup(
            &mut self,
            _key: Option<T::Key>,
            _subkey: Option<T::SubKey>,
        ) -> Result<DupWalker<'_, T, Self>, DatabaseError> {
            unreachable!("unused by prefix seek tests")
        }
    }

    impl<T, C> DbCursorRO<T> for RecordingCursor<C>
    where
        T: Table,
        C: DbCursorRO<T>,
    {
        fn first(&mut self) -> PairResult<T> {
            self.inner.first()
        }

        fn seek_exact(&mut self, key: T::Key) -> PairResult<T> {
            self.calls.seek_exact += 1;
            self.inner.seek_exact(key)
        }

        fn seek_exact_key(&mut self, key: T::Key) -> KeyOnlyResult<T> {
            self.inner.seek_exact_key(key)
        }

        fn seek(&mut self, key: T::Key) -> PairResult<T> {
            self.calls.seek += 1;
            self.inner.seek(key)
        }

        fn seek_key(&mut self, key: T::Key) -> KeyOnlyResult<T> {
            self.inner.seek_key(key)
        }

        fn next(&mut self) -> PairResult<T> {
            self.calls.next += 1;
            self.inner.next()
        }

        fn next_key(&mut self) -> KeyOnlyResult<T> {
            self.calls.next_key += 1;
            self.inner.next_key()
        }

        fn prev(&mut self) -> PairResult<T> {
            self.inner.prev()
        }

        fn last(&mut self) -> PairResult<T> {
            self.inner.last()
        }

        fn current(&mut self) -> PairResult<T> {
            self.calls.current += 1;
            self.inner.current()
        }

        fn walk(
            &mut self,
            start_key: Option<T::Key>,
        ) -> Result<Walker<'_, T, Self>, DatabaseError> {
            let start =
                if let Some(start_key) = start_key { self.seek(start_key) } else { self.first() }
                    .transpose();
            Ok(Walker::new(self, start))
        }

        fn walk_range(
            &mut self,
            range: impl RangeBounds<T::Key>,
        ) -> Result<RangeWalker<'_, T, Self>, DatabaseError> {
            let start = match range.start_bound().cloned() {
                Bound::Included(key) => self.seek(key),
                Bound::Excluded(_) => {
                    unreachable!("Rust doesn't allow excluded starting bounds")
                }
                Bound::Unbounded => self.first(),
            }
            .transpose();
            Ok(RangeWalker::new(self, start, range.end_bound().cloned()))
        }

        fn walk_back(
            &mut self,
            start_key: Option<T::Key>,
        ) -> Result<ReverseWalker<'_, T, Self>, DatabaseError> {
            let start =
                if let Some(start_key) = start_key { self.seek(start_key) } else { self.last() }
                    .transpose();
            Ok(ReverseWalker::new(self, start))
        }
    }

    fn take_calls<C, A: TrieKeyAdapter>(
        cursor: &mut DatabaseAccountTrieCursor<RecordingCursor<C>, A>,
    ) -> CursorCalls {
        std::mem::take(&mut cursor.cursor.calls)
    }

    fn take_storage_calls<C, A: TrieKeyAdapter>(
        cursor: &mut DatabaseStorageTrieCursor<RecordingCursor<C>, A>,
    ) -> CursorCalls {
        std::mem::take(&mut cursor.cursor.calls)
    }

    fn assert_account_trie_prefix_seek<A: TrieTableAdapter>() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let paths = [
            Nibbles::from_nibbles([0x1]),
            Nibbles::from_nibbles([0x1, 0x1]),
            Nibbles::from_nibbles([0x1, 0x3]),
            Nibbles::from_nibbles([0x1, 0x4]),
        ];
        let nodes = [1, 2, 4, 8].map(|mask| BranchNodeCompact::new(mask, mask, 0, vec![], None));

        {
            let mut cursor = provider.tx_ref().cursor_write::<A::AccountTrieTable>().unwrap();
            for (path, node) in paths.into_iter().zip(&nodes) {
                cursor.upsert(A::AccountKey::from(path), node).unwrap();
            }
        }

        let db_cursor = provider.tx_ref().cursor_read::<A::AccountTrieTable>().unwrap();
        let mut cursor = DatabaseAccountTrieCursor::<_, A>::new(RecordingCursor::new(db_cursor));

        assert_eq!(cursor.seek(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        take_calls(&mut cursor);

        let between = Nibbles::from_nibbles([0x1, 0x2]);
        assert_eq!(cursor.seek(between).unwrap(), Some((paths[2], nodes[2].clone())));
        assert_eq!(
            take_calls(&mut cursor),
            CursorCalls { next_key: 2, current: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek_exact(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        assert_eq!(take_calls(&mut cursor), CursorCalls { seek_exact: 1, ..Default::default() });

        assert_eq!(cursor.seek_exact(paths[2]).unwrap(), Some((paths[2], nodes[2].clone())));
        assert_eq!(
            take_calls(&mut cursor),
            CursorCalls { next_key: 2, current: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        assert_eq!(take_calls(&mut cursor), CursorCalls { seek: 1, ..Default::default() });

        assert_eq!(cursor.seek(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        assert_eq!(take_calls(&mut cursor), CursorCalls { current: 1, ..Default::default() });

        assert_eq!(cursor.seek_exact(between).unwrap(), None);
        assert_eq!(take_calls(&mut cursor), CursorCalls { next_key: 2, ..Default::default() });
        assert_eq!(cursor.current().unwrap(), Some(paths[2]));
        take_calls(&mut cursor);

        assert_eq!(cursor.seek_exact(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        take_calls(&mut cursor);
        let after_descendants = Nibbles::from_nibbles([0x1, 0xf]);
        assert_eq!(cursor.seek_exact(after_descendants).unwrap(), None);
        assert_eq!(take_calls(&mut cursor), CursorCalls { next_key: 4, ..Default::default() });

        assert_eq!(cursor.seek(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        take_calls(&mut cursor);
        assert_eq!(cursor.next().unwrap(), Some((paths[1], nodes[1].clone())));
        assert_eq!(take_calls(&mut cursor), CursorCalls { next: 1, ..Default::default() });
        assert_eq!(cursor.seek(paths[1]).unwrap(), Some((paths[1], nodes[1].clone())));
        assert_eq!(take_calls(&mut cursor), CursorCalls { current: 1, ..Default::default() });

        assert_eq!(cursor.seek(paths[3]).unwrap(), Some((paths[3], nodes[3].clone())));
        take_calls(&mut cursor);
        let after_last = Nibbles::from_nibbles([0x1, 0x4, 0xf]);
        assert_eq!(cursor.seek(after_last).unwrap(), None);
        assert_eq!(take_calls(&mut cursor), CursorCalls { next_key: 1, ..Default::default() });

        assert_eq!(cursor.seek(paths[0]).unwrap().map(|(key, _)| key), Some(paths[0]));
        cursor.reset();
        take_calls(&mut cursor);
        assert_eq!(cursor.seek(between).unwrap().map(|(key, _)| key), Some(paths[2]));
        assert_eq!(take_calls(&mut cursor), CursorCalls { seek: 1, ..Default::default() });
    }

    #[test]
    fn test_account_trie_prefix_seek_legacy() {
        assert_account_trie_prefix_seek::<LegacyKeyAdapter>();
    }

    #[test]
    fn test_account_trie_prefix_seek_packed() {
        assert_account_trie_prefix_seek::<PackedKeyAdapter>();
    }

    fn assert_storage_trie_prefix_seek<A: TrieTableAdapter>() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let hashed_address = B256::with_last_byte(1);
        let next_hashed_address = B256::with_last_byte(2);
        let paths = [
            Nibbles::from_nibbles([0x1]),
            Nibbles::from_nibbles([0x1, 0x1]),
            Nibbles::from_nibbles([0x1, 0x3]),
            Nibbles::from_nibbles([0x1, 0x4]),
        ];
        let nodes = [1, 2, 4, 8].map(|mask| BranchNodeCompact::new(mask, mask, 0, vec![], None));
        let next_node = BranchNodeCompact::new(16, 16, 0, vec![], None);

        {
            let mut cursor = provider.tx_ref().cursor_dup_write::<A::StorageTrieTable>().unwrap();
            for (path, node) in paths.into_iter().zip(&nodes) {
                cursor
                    .upsert(
                        hashed_address,
                        &A::StorageValue::new(A::StorageSubKey::from(path), node.clone()),
                    )
                    .unwrap();
            }
            cursor
                .upsert(
                    next_hashed_address,
                    &A::StorageValue::new(A::StorageSubKey::from(paths[0]), next_node.clone()),
                )
                .unwrap();
        }

        let db_cursor = provider.tx_ref().cursor_dup_read::<A::StorageTrieTable>().unwrap();
        let mut cursor =
            DatabaseStorageTrieCursor::<_, A>::new(RecordingCursor::new(db_cursor), hashed_address);

        assert_eq!(cursor.seek(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        take_storage_calls(&mut cursor);

        let between = Nibbles::from_nibbles([0x1, 0x2]);
        assert_eq!(cursor.seek(between).unwrap(), Some((paths[2], nodes[2].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup_key: 2, current: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek_exact(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { seek_by_key_subkey: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek_exact(paths[2]).unwrap(), Some((paths[2], nodes[2].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup_key: 2, current: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { seek_by_key_subkey: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { current: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek_exact(between).unwrap(), None);
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup_key: 2, ..Default::default() }
        );
        assert_eq!(cursor.current().unwrap(), Some(paths[2]));
        take_storage_calls(&mut cursor);

        cursor.reset();
        assert_eq!(cursor.seek_exact(between).unwrap(), None);
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { seek_by_key_subkey: 1, ..Default::default() }
        );
        let after_overshoot = Nibbles::from_nibbles([0x1, 0x3, 0x0]);
        assert_eq!(cursor.seek_exact(after_overshoot).unwrap(), None);
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup_key: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek_exact(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        take_storage_calls(&mut cursor);
        let after_descendants = Nibbles::from_nibbles([0x1, 0xf]);
        assert_eq!(cursor.seek_exact(after_descendants).unwrap(), None);
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup_key: 4, ..Default::default() }
        );

        assert_eq!(cursor.seek(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        take_storage_calls(&mut cursor);
        assert_eq!(cursor.next().unwrap(), Some((paths[1], nodes[1].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup: 1, ..Default::default() }
        );
        assert_eq!(cursor.seek(paths[1]).unwrap(), Some((paths[1], nodes[1].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { current: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek(paths[3]).unwrap(), Some((paths[3], nodes[3].clone())));
        take_storage_calls(&mut cursor);
        let after_last = Nibbles::from_nibbles([0x1, 0x4, 0xf]);
        assert_eq!(cursor.seek(after_last).unwrap(), None);
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { next_dup_key: 1, ..Default::default() }
        );

        assert_eq!(cursor.seek(paths[0]).unwrap(), Some((paths[0], nodes[0].clone())));
        cursor.reset();
        take_storage_calls(&mut cursor);
        assert_eq!(cursor.seek(between).unwrap(), Some((paths[2], nodes[2].clone())));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { seek_by_key_subkey: 1, ..Default::default() }
        );

        cursor.set_hashed_address(next_hashed_address);
        take_storage_calls(&mut cursor);
        assert_eq!(cursor.seek(paths[0]).unwrap(), Some((paths[0], next_node)));
        assert_eq!(
            take_storage_calls(&mut cursor),
            CursorCalls { seek_by_key_subkey: 1, ..Default::default() }
        );
    }

    #[test]
    fn test_storage_trie_prefix_seek_legacy() {
        assert_storage_trie_prefix_seek::<LegacyKeyAdapter>();
    }

    #[test]
    fn test_storage_trie_prefix_seek_packed() {
        assert_storage_trie_prefix_seek::<PackedKeyAdapter>();
    }

    #[test]
    fn test_account_trie_order() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();

        let data = vec![
            hex!("0303040e").to_vec(),
            hex!("030305").to_vec(),
            hex!("03030500").to_vec(),
            hex!("0303050a").to_vec(),
        ];

        for key in data.clone() {
            cursor
                .upsert(
                    key.into(),
                    &BranchNodeCompact::new(
                        0b0000_0010_0000_0001,
                        0b0000_0010_0000_0001,
                        0,
                        Vec::default(),
                        None,
                    ),
                )
                .unwrap();
        }

        let db_data = cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(db_data[0].0 .0.to_vec(), data[0]);
        assert_eq!(db_data[1].0 .0.to_vec(), data[1]);
        assert_eq!(db_data[2].0 .0.to_vec(), data[2]);
        assert_eq!(db_data[3].0 .0.to_vec(), data[3]);

        assert_eq!(
            cursor.seek(hex!("0303040f").to_vec().into()).unwrap().map(|(k, _)| k.0.to_vec()),
            Some(data[1].clone())
        );
    }

    // tests that upsert and seek match on the storage trie cursor
    #[test]
    fn test_storage_cursor_abstraction() {
        use reth_storage_api::StorageSettingsCache;
        use reth_trie::trie_cursor::{TrieCursor, TrieCursorFactory};

        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let hashed_address = B256::random();
        let key = StoredNibblesSubKey::from(vec![0x2, 0x3]);
        let value = BranchNodeCompact::new(1, 1, 1, vec![B256::random()], None);

        cursor
            .upsert(hashed_address, &StorageTrieEntry { nibbles: key.clone(), node: value.clone() })
            .unwrap();

        crate::with_adapter!(provider, |A| {
            let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
            let mut cursor = trie_factory.storage_trie_cursor(hashed_address).unwrap();
            assert_eq!(cursor.seek(key.into()).unwrap().unwrap().1, value);
        });
    }
}
