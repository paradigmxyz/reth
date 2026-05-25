use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use alloy_primitives::{map::B256Map, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostStateSorted;
use std::{
    marker::PhantomData,
    ops::{Deref, Index},
    sync::Arc,
};

/// The hashed cursor factory for the post state.
#[derive(Clone, Debug)]
pub struct HashedPostStateCursorFactory<'overlay, CF, T> {
    cursor_factory: CF,
    post_state: T,
    _marker: PhantomData<&'overlay HashedPostStateSorted>,
}

impl<'overlay, CF, T> HashedPostStateCursorFactory<'overlay, CF, T> {
    /// Create a new factory.
    pub const fn new(cursor_factory: CF, post_state: T) -> Self {
        Self { cursor_factory, post_state, _marker: PhantomData }
    }
}

impl<'overlay, CF, T> HashedCursorFactory for HashedPostStateCursorFactory<'overlay, CF, T>
where
    CF: HashedCursorFactory + 'overlay,
    T: AsRef<[&'overlay HashedPostStateSorted]>,
{
    type AccountCursor<'cursor>
        = HashedPostStateCursor<'overlay, CF::AccountCursor<'cursor>, Option<Account>>
    where
        Self: 'cursor;
    type StorageCursor<'cursor>
        = HashedPostStateCursor<'overlay, CF::StorageCursor<'cursor>, U256>
    where
        Self: 'cursor;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.hashed_account_cursor()?;
        Ok(HashedPostStateCursor::new_account(cursor, self.post_state.as_ref().iter().copied()))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.hashed_storage_cursor(hashed_address)?;
        Ok(HashedPostStateCursor::new_storage(
            cursor,
            self.post_state.as_ref().iter().copied(),
            hashed_address,
        ))
    }
}

/// Trait for types that can be used with [`HashedPostStateCursor`] as a value.
///
/// This enables uniform handling of deletions across different wrapper types:
/// - `Option<Account>`: `None` indicates deletion
/// - `U256`: `U256::ZERO` indicates deletion (maps to `None`)
///
/// This design allows us to use `U256::ZERO`, rather than an Option, to indicate deletion for
/// storage (which maps cleanly to how changesets are stored in the DB) while not requiring two
/// different cursor implementations.
pub trait HashedPostStateCursorValue: Copy {
    /// The non-zero type returned by `into_option`.
    /// For `Option<Account>`, this is `Account`.
    /// For `U256`, this is `U256`.
    type NonZero: Copy + std::fmt::Debug;

    /// Returns `Some(&NonZero)` if the value is present, `None` if deleted.
    fn into_option(self) -> Option<Self::NonZero>;
}

impl HashedPostStateCursorValue for Option<Account> {
    type NonZero = Account;

    fn into_option(self) -> Option<Self::NonZero> {
        self
    }
}

impl HashedPostStateCursorValue for U256 {
    type NonZero = Self;

    fn into_option(self) -> Option<Self::NonZero> {
        (!self.is_zero()).then_some(self)
    }
}

/// Hashed post-state overlays ordered from highest to lowest precedence.
#[derive(Clone, Debug, Default)]
pub struct HashedPostStateOverlay {
    states: Vec<Arc<HashedPostStateSorted>>,
    storage_index: Arc<B256Map<StorageOverlayIndex>>,
}

impl HashedPostStateOverlay {
    /// Create a new indexed hashed post-state overlay stack.
    pub fn new(states: Vec<Arc<HashedPostStateSorted>>) -> Self {
        let storage_index = Arc::new(build_hashed_storage_index(&states));
        Self { states, storage_index }
    }

    /// Returns `true` if there are no hashed post-state overlays.
    pub const fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Returns the number of hashed post-state overlays.
    pub const fn len(&self) -> usize {
        self.states.len()
    }

    /// Returns an iterator over hashed post-state overlays.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<HashedPostStateSorted>> {
        self.states.iter()
    }

    /// Push a hashed post-state overlay at the end of the precedence stack.
    pub fn push(&mut self, state: Arc<HashedPostStateSorted>) {
        self.states.push(state);
        self.rebuild_storage_index();
    }

    /// Insert a hashed post-state overlay at `index`.
    pub fn insert(&mut self, index: usize, state: Arc<HashedPostStateSorted>) {
        self.states.insert(index, state);
        self.rebuild_storage_index();
    }

    fn storage_overlay(&self, hashed_address: B256) -> (PostStateOverlayCursor<'_, U256>, bool) {
        let Some(index) = self.storage_index.get(&hashed_address) else {
            return (PostStateOverlayCursor::default(), false);
        };

        (
            PostStateOverlayCursor {
                cursors: index
                    .indices
                    .iter()
                    .filter_map(|idx| self.states[*idx].storages.get(&hashed_address))
                    .map(|storage| SeekablePostStateCursor::new(storage.storage_slots_ref()))
                    .collect(),
            },
            index.db_wiped,
        )
    }

    fn rebuild_storage_index(&mut self) {
        self.storage_index = Arc::new(build_hashed_storage_index(&self.states));
    }
}

impl From<Vec<Arc<HashedPostStateSorted>>> for HashedPostStateOverlay {
    fn from(states: Vec<Arc<HashedPostStateSorted>>) -> Self {
        Self::new(states)
    }
}

impl IntoIterator for HashedPostStateOverlay {
    type IntoIter = std::vec::IntoIter<Self::Item>;
    type Item = Arc<HashedPostStateSorted>;

    fn into_iter(self) -> Self::IntoIter {
        self.states.into_iter()
    }
}

impl Index<usize> for HashedPostStateOverlay {
    type Output = Arc<HashedPostStateSorted>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.states[index]
    }
}

impl Deref for HashedPostStateOverlay {
    type Target = [Arc<HashedPostStateSorted>];

    fn deref(&self) -> &Self::Target {
        &self.states
    }
}

#[derive(Clone, Debug)]
struct StorageOverlayIndex {
    indices: Arc<[usize]>,
    db_wiped: bool,
}

#[derive(Default)]
struct StorageOverlayIndexBuilder {
    indices: Vec<usize>,
    db_wiped: bool,
}

fn build_hashed_storage_index(
    states: &[Arc<HashedPostStateSorted>],
) -> B256Map<StorageOverlayIndex> {
    let mut index: B256Map<StorageOverlayIndexBuilder> = B256Map::default();

    for (idx, state) in states.iter().enumerate() {
        for (hashed_address, storage) in &state.storages {
            let entry = index.entry(*hashed_address).or_default();
            if entry.db_wiped {
                continue;
            }

            entry.indices.push(idx);
            if storage.is_wiped() {
                entry.db_wiped = true;
            }
        }
    }

    index
        .into_iter()
        .map(|(hashed_address, entry)| {
            (
                hashed_address,
                StorageOverlayIndex { indices: entry.indices.into(), db_wiped: entry.db_wiped },
            )
        })
        .collect()
}

#[derive(Clone, Debug)]
enum HashedPostStateSource<'a> {
    Refs(Vec<&'a HashedPostStateSorted>),
    Indexed(&'a HashedPostStateOverlay),
}

impl<'a> HashedPostStateSource<'a> {
    fn from_refs(post_states: impl IntoIterator<Item = &'a HashedPostStateSorted>) -> Self {
        Self::Refs(post_states.into_iter().collect())
    }

    fn account_overlay(&self) -> PostStateOverlayCursor<'a, Option<Account>> {
        match self {
            Self::Refs(post_states) => PostStateOverlayCursor::account(post_states),
            Self::Indexed(post_states) => PostStateOverlayCursor {
                cursors: post_states
                    .iter()
                    .map(|post_state| SeekablePostStateCursor::new(post_state.accounts.as_slice()))
                    .collect(),
            },
        }
    }

    fn storage_overlay(&self, hashed_address: B256) -> (PostStateOverlayCursor<'a, U256>, bool) {
        match self {
            Self::Refs(post_states) => PostStateOverlayCursor::storage(post_states, hashed_address),
            Self::Indexed(post_states) => post_states.storage_overlay(hashed_address),
        }
    }
}

/// A cursor to iterate over state updates and corresponding database entries.
/// It will always give precedence to earlier post state overlays.
#[derive(Debug)]
pub struct HashedPostStateCursor<'a, C, V>
where
    V: HashedPostStateCursorValue,
{
    /// The underlying cursor.
    cursor: C,
    /// The current DB cursor state.
    db_cursor_state: DbCursorState<V::NonZero>,
    /// In-memory cursors over post state overlays.
    post_state_cursor: PostStateOverlayCursor<'a, V>,
    /// Lower-priority overlays that still need positioning after a lazy exact overlay hit.
    deferred_overlay_seek_start: Option<usize>,
    /// The last hashed key that was returned by the cursor.
    /// De facto, this is a current cursor position.
    last_key: Option<B256>,
    #[cfg(debug_assertions)]
    /// Tracks whether `seek` has been called.
    seeked: bool,
    /// Source of post-state overlays.
    post_states: HashedPostStateSource<'a>,
}

#[derive(Debug)]
enum DbCursorState<V> {
    Unpositioned,
    Positioned((B256, V)),
    Wiped,
}

impl<V> DbCursorState<V> {
    const fn new(cursor_wiped: bool) -> Self {
        if cursor_wiped {
            Self::Wiped
        } else {
            Self::Unpositioned
        }
    }

    const fn is_wiped(&self) -> bool {
        matches!(self, Self::Wiped)
    }

    const fn entry(&self) -> Option<&(B256, V)> {
        match self {
            Self::Positioned(entry) => Some(entry),
            Self::Unpositioned | Self::Wiped => None,
        }
    }

    fn set_entry(&mut self, entry: Option<(B256, V)>) {
        if !self.is_wiped() {
            *self = entry.map(Self::Positioned).unwrap_or(Self::Unpositioned);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct PostStateOverlayCursor<'a, V> {
    cursors: Vec<SeekablePostStateCursor<'a, V>>,
}

impl<'a> PostStateOverlayCursor<'a, Option<Account>> {
    fn account(post_states: &[&'a HashedPostStateSorted]) -> Self {
        Self {
            cursors: post_states
                .iter()
                .map(|post_state| SeekablePostStateCursor::new(post_state.accounts.as_slice()))
                .collect(),
        }
    }
}

impl<'a> PostStateOverlayCursor<'a, U256> {
    fn storage(post_states: &[&'a HashedPostStateSorted], hashed_address: B256) -> (Self, bool) {
        let mut cursors = Vec::new();
        let mut db_wiped = false;

        for post_state in post_states {
            if let Some(storage) = post_state.storages.get(&hashed_address) {
                cursors.push(SeekablePostStateCursor::new(storage.storage_slots_ref()));
                if storage.is_wiped() {
                    db_wiped = true;
                    break;
                }
            }
        }

        (Self { cursors }, db_wiped)
    }
}

impl<'a, V> PostStateOverlayCursor<'a, V>
where
    V: HashedPostStateCursorValue,
{
    fn seek_from(&mut self, start: usize, key: &B256) {
        for cursor in self.cursors.iter_mut().skip(start) {
            cursor.seek(key);
        }
    }

    fn seek_until_exact(&mut self, key: &B256) -> Option<(usize, Option<V::NonZero>)> {
        for (idx, cursor) in self.cursors.iter_mut().enumerate() {
            if let Some((cursor_key, value)) = cursor.seek(key) &&
                cursor_key == key
            {
                return Some((idx, value.into_option()))
            }
        }
        None
    }

    fn first_after(&mut self, key: &B256) {
        for cursor in &mut self.cursors {
            cursor.first_after(key);
        }
    }

    fn reset(&mut self) {
        for cursor in &mut self.cursors {
            cursor.reset();
        }
    }

    fn min_current_key(&self) -> Option<B256> {
        self.cursors.iter().filter_map(|cursor| cursor.current().map(|(key, _)| *key)).min()
    }

    fn highest_priority_value_at(&self, key: &B256) -> Option<Option<V::NonZero>> {
        self.cursors.iter().find_map(|cursor| {
            let (cursor_key, value) = cursor.current()?;
            (cursor_key == key).then(|| value.into_option())
        })
    }

    fn advance_key(&mut self, key: &B256) {
        for cursor in &mut self.cursors {
            if cursor.current().is_some_and(|(cursor_key, _)| cursor_key == key) {
                cursor.first_after(key);
            }
        }
    }

    fn has_visible_value(&self) -> bool {
        let mut cursor = self.clone();
        cursor.reset();
        while let Some(key) = cursor.min_current_key() {
            if cursor.highest_priority_value_at(&key).flatten().is_some() {
                return true
            }
            cursor.advance_key(&key);
        }
        false
    }
}

#[derive(Clone, Debug)]
struct SeekablePostStateCursor<'a, V> {
    entries: &'a [(B256, V)],
    idx: usize,
}

impl<'a, V> SeekablePostStateCursor<'a, V> {
    const fn new(entries: &'a [(B256, V)]) -> Self {
        Self { entries, idx: 0 }
    }

    fn current(&self) -> Option<&'a (B256, V)> {
        self.entries.get(self.idx)
    }

    const fn reset(&mut self) {
        self.idx = 0;
    }

    fn seek(&mut self, key: &B256) -> Option<&'a (B256, V)> {
        self.idx = self.entries.partition_point(|(entry_key, _)| entry_key < key);
        self.current()
    }

    fn first_after(&mut self, key: &B256) -> Option<&'a (B256, V)> {
        if self.current().is_some_and(|(entry_key, _)| entry_key > key) {
            return self.current()
        }

        let remaining = &self.entries[self.idx..];
        self.idx += remaining.partition_point(|(entry_key, _)| entry_key <= key);
        self.current()
    }
}

impl<'a, C> HashedPostStateCursor<'a, C, Option<Account>>
where
    C: HashedCursor<Value = Account>,
{
    /// Create new account cursor which combines a DB cursor and the post state.
    pub fn new_account(
        cursor: C,
        post_states: impl IntoIterator<Item = &'a HashedPostStateSorted>,
    ) -> Self {
        let post_states = HashedPostStateSource::from_refs(post_states);
        let post_state_cursor = post_states.account_overlay();
        Self {
            cursor,
            db_cursor_state: DbCursorState::new(false),
            post_state_cursor,
            deferred_overlay_seek_start: None,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
            post_states,
        }
    }
}

impl<'a, C> HashedPostStateCursor<'a, C, Option<Account>>
where
    C: HashedCursor<Value = Account>,
{
    /// Create new account cursor from an indexed hashed post-state overlay.
    pub fn new_account_from_overlay(cursor: C, post_states: &'a HashedPostStateOverlay) -> Self {
        let post_states = HashedPostStateSource::Indexed(post_states);
        let post_state_cursor = post_states.account_overlay();
        Self {
            cursor,
            db_cursor_state: DbCursorState::new(false),
            post_state_cursor,
            deferred_overlay_seek_start: None,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
            post_states,
        }
    }
}

impl<'a, C> HashedPostStateCursor<'a, C, U256>
where
    C: HashedStorageCursor<Value = U256>,
{
    /// Create new storage cursor with full post state reference.
    /// This allows the cursor to switch between storage tries when `set_hashed_address` is called.
    pub fn new_storage(
        cursor: C,
        post_states: impl IntoIterator<Item = &'a HashedPostStateSorted>,
        hashed_address: B256,
    ) -> Self {
        let post_states = HashedPostStateSource::from_refs(post_states);
        let (post_state_cursor, cursor_wiped) = post_states.storage_overlay(hashed_address);
        Self {
            cursor,
            db_cursor_state: DbCursorState::new(cursor_wiped),
            post_state_cursor,
            deferred_overlay_seek_start: None,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
            post_states,
        }
    }

    /// Create new storage cursor from an indexed hashed post-state overlay.
    pub fn new_storage_from_overlay(
        cursor: C,
        post_states: &'a HashedPostStateOverlay,
        hashed_address: B256,
    ) -> Self {
        let post_states = HashedPostStateSource::Indexed(post_states);
        let (post_state_cursor, cursor_wiped) = post_states.storage_overlay(hashed_address);
        Self {
            cursor,
            db_cursor_state: DbCursorState::new(cursor_wiped),
            post_state_cursor,
            deferred_overlay_seek_start: None,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
            post_states,
        }
    }
}

impl<'a, C, V> HashedPostStateCursor<'a, C, V>
where
    C: HashedCursor<Value = V::NonZero>,
    V: HashedPostStateCursorValue,
{
    /// Returns a mutable reference to the underlying cursor if it's not wiped, None otherwise.
    fn get_cursor_mut(&mut self) -> Option<&mut C> {
        (!self.db_cursor_state.is_wiped()).then_some(&mut self.cursor)
    }

    fn set_last_key(&mut self, next_entry: &Option<(B256, V::NonZero)>) {
        self.last_key = next_entry.as_ref().map(|e| e.0);
    }

    /// Positions the DB cursor state using the underlying cursor.
    fn cursor_seek(&mut self, key: B256) -> Result<(), DatabaseError> {
        let entry = self.get_cursor_mut().map(|c| c.seek(key)).transpose()?.flatten();
        self.db_cursor_state.set_entry(entry);
        Ok(())
    }

    /// Positions the DB cursor at the first entry after `key`.
    fn cursor_first_after(&mut self, key: B256) -> Result<(), DatabaseError> {
        self.cursor_seek(key)?;
        if self.db_cursor_state.entry().is_some_and(|(db_key, _)| db_key == &key) {
            self.cursor_next()?;
        }
        Ok(())
    }

    /// Advances the DB cursor state to the subsequent entry using the underlying cursor.
    fn cursor_next(&mut self) -> Result<(), DatabaseError> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.seeked);
        }

        let entry = self.get_cursor_mut().map(|c| c.next()).transpose()?.flatten();
        self.db_cursor_state.set_entry(entry);

        Ok(())
    }

    /// Performs a k-way merge over the positioned overlay cursors and the DB cursor.
    fn choose_next_entry(&mut self) -> Result<Option<(B256, V::NonZero)>, DatabaseError> {
        loop {
            let mem_key = self.post_state_cursor.min_current_key();
            let db_key = self.db_cursor_state.entry().map(|(key, _)| *key);
            let Some(next_key) = mem_key.into_iter().chain(db_key).min() else {
                return Ok(None);
            };

            if let Some(mem_value) = self.post_state_cursor.highest_priority_value_at(&next_key) {
                if let Some(value) = mem_value {
                    return Ok(Some((next_key, value)))
                }

                self.post_state_cursor.advance_key(&next_key);
                if self.db_cursor_state.entry().is_some_and(|(db_key, _)| db_key == &next_key) {
                    self.cursor_next()?;
                }
                continue;
            }

            if self.db_cursor_state.entry().is_some_and(|(db_key, _)| db_key == &next_key) {
                return Ok(self.db_cursor_state.entry().copied())
            }
        }
    }
}

impl<C, V> HashedCursor for HashedPostStateCursor<'_, C, V>
where
    C: HashedCursor<Value = V::NonZero>,
    V: HashedPostStateCursorValue,
{
    type Value = V::NonZero;

    /// Seek the next entry for a given hashed key.
    ///
    /// If the post state contains the exact match for the key, return it.
    /// Otherwise, retrieve the next entries that are greater than or equal to the key from the
    /// database and the post state. The two entries are compared and the lowest is returned.
    ///
    /// The returned account key is memoized and the cursor remains positioned at that key until
    /// [`HashedCursor::seek`] or [`HashedCursor::next`] are called.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        #[cfg(debug_assertions)]
        {
            self.seeked = true;
        }

        self.deferred_overlay_seek_start = None;
        match self.post_state_cursor.seek_until_exact(&key) {
            Some((idx, Some(value))) => {
                let entry = Some((key, value));
                self.deferred_overlay_seek_start = Some(idx + 1);
                self.set_last_key(&entry);
                return Ok(entry)
            }
            Some((idx, None)) => {
                self.post_state_cursor.seek_from(idx + 1, &key);
            }
            None => {}
        }

        self.cursor_seek(key)?;

        let entry = self.choose_next_entry()?;
        self.set_last_key(&entry);
        Ok(entry)
    }

    /// Retrieve the next entry from the cursor.
    ///
    /// If the cursor is positioned at the entry, return the entry with next greater key.
    /// Returns [None] if the previous memoized or the next greater entries are missing.
    ///
    /// NOTE: This function will not return any entry unless [`HashedCursor::seek`] has been called.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.seeked, "Cursor must be seek'd before next is called");
        }

        // A `last_key` of `None` indicates that the cursor is exhausted.
        let Some(last_key) = self.last_key else {
            return Ok(None);
        };

        if let Some(start) = self.deferred_overlay_seek_start.take() {
            self.post_state_cursor.seek_from(start, &last_key);
        }
        self.post_state_cursor.first_after(&last_key);

        if self.db_cursor_state.entry().is_some_and(|(db_key, _)| db_key == &last_key) {
            self.cursor_next()?;
        } else {
            self.cursor_first_after(last_key)?;
        }

        let entry = self.choose_next_entry()?;
        self.set_last_key(&entry);
        Ok(entry)
    }

    fn reset(&mut self) {
        self.cursor.reset();
        self.post_state_cursor.reset();

        self.db_cursor_state.set_entry(None);
        self.deferred_overlay_seek_start = None;
        self.last_key = None;
        #[cfg(debug_assertions)]
        {
            self.seeked = false;
        }
    }
}

/// The cursor to iterate over post state hashed values and corresponding database entries.
/// It will always give precedence to the data from the post state.
impl<C> HashedStorageCursor for HashedPostStateCursor<'_, C, U256>
where
    C: HashedStorageCursor<Value = U256>,
{
    /// Returns `true` if the account has no storage entries.
    ///
    /// This function should be called before attempting to call [`HashedCursor::seek`] or
    /// [`HashedCursor::next`].
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        // Storage is not empty if it has non-zero slots.
        if self.post_state_cursor.has_visible_value() {
            return Ok(false);
        }

        // If no non-zero slots in post state, check the database.
        // Returns true if cursor is wiped.
        self.get_cursor_mut().map_or(Ok(true), |c| c.is_storage_empty())
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.reset();
        self.cursor.set_hashed_address(hashed_address);
        let (post_state_cursor, cursor_wiped) = self.post_states.storage_overlay(hashed_address);
        self.post_state_cursor = post_state_cursor;
        self.db_cursor_state = DbCursorState::new(cursor_wiped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashed_cursor::mock::MockHashedCursor;
    use parking_lot::Mutex;
    use std::{collections::BTreeMap, sync::Arc};

    fn key(byte: u8) -> B256 {
        B256::repeat_byte(byte)
    }

    fn account(nonce: u64) -> Account {
        Account { nonce, balance: U256::from(nonce), bytecode_hash: None }
    }

    fn storage_post_state(storage_slots: Vec<(B256, U256)>) -> HashedPostStateSorted {
        storage_post_state_for_address(B256::ZERO, storage_slots)
    }

    fn storage_post_state_with_wipe(
        storage_slots: Vec<(B256, U256)>,
        wiped: bool,
    ) -> HashedPostStateSorted {
        storage_post_state_with_wipe_for_address(B256::ZERO, storage_slots, wiped)
    }

    fn storage_post_state_for_address(
        hashed_address: B256,
        storage_slots: Vec<(B256, U256)>,
    ) -> HashedPostStateSorted {
        storage_post_state_with_wipe_for_address(hashed_address, storage_slots, false)
    }

    fn storage_post_state_with_wipe_for_address(
        hashed_address: B256,
        storage_slots: Vec<(B256, U256)>,
        wiped: bool,
    ) -> HashedPostStateSorted {
        let storage_sorted = reth_trie_common::HashedStorageSorted { storage_slots, wiped };
        let mut storages = alloy_primitives::map::B256Map::default();
        storages.insert(hashed_address, storage_sorted);
        HashedPostStateSorted::new(Vec::new(), storages)
    }

    #[test]
    fn test_seek_overlay_exact_hit_does_not_touch_db_until_next() {
        let db_nodes = vec![(key(0x02), U256::from(2)), (key(0x03), U256::from(3))];
        let post_state_nodes = vec![(key(0x02), U256::from(42))];

        let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let post_state = storage_post_state(post_state_nodes);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, [&post_state], B256::ZERO);

        let result = cursor.seek(key(0x02)).unwrap();
        assert_eq!(result, Some((key(0x02), U256::from(42))));
        assert!(visited_keys.lock().is_empty(), "exact overlay hit should not touch the DB cursor");

        let result = cursor.next().unwrap();
        assert_eq!(result, Some((key(0x03), U256::from(3))));
        assert!(!visited_keys.lock().is_empty(), "next should lazily position the DB cursor");
    }

    #[test]
    fn test_seek_overlay_exact_hit_repositions_stale_db_on_next() {
        let db_nodes = vec![(key(0x01), U256::from(1)), (key(0x03), U256::from(3))];
        let post_state_nodes = vec![(key(0x02), U256::from(2))];

        let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let post_state = storage_post_state(post_state_nodes);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, [&post_state], B256::ZERO);

        let result = cursor.seek(key(0x01)).unwrap();
        assert_eq!(result, Some((key(0x01), U256::from(1))));
        assert_eq!(visited_keys.lock().len(), 1);

        let result = cursor.seek(key(0x02)).unwrap();
        assert_eq!(result, Some((key(0x02), U256::from(2))));
        assert_eq!(visited_keys.lock().len(), 1, "exact overlay hit should not seek the DB");

        let result = cursor.next().unwrap();
        assert_eq!(result, Some((key(0x03), U256::from(3))));
    }

    #[test]
    fn test_seek_overlay_exact_deletion_still_seeks_db() {
        let db_nodes = vec![(key(0x02), U256::from(2)), (key(0x03), U256::from(3))];
        let post_state_nodes = vec![(key(0x02), U256::ZERO)];

        let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let post_state = storage_post_state(post_state_nodes);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, [&post_state], B256::ZERO);

        let result = cursor.seek(key(0x02)).unwrap();
        assert_eq!(result, Some((key(0x03), U256::from(3))));
        assert!(!visited_keys.lock().is_empty(), "exact overlay deletion should consult the DB");
    }

    #[test]
    fn test_seek_overlay_exact_hit_does_not_seek_lower_overlays_or_db() {
        let db_nodes = BTreeMap::from([(key(0x06), U256::from(6))]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let higher_priority =
            storage_post_state(vec![(key(0x01), U256::from(1)), (key(0x09), U256::from(9))]);
        let exact_hit = storage_post_state(vec![(key(0x05), U256::from(5))]);
        let lower_priority =
            storage_post_state(vec![(key(0x01), U256::from(10)), (key(0x07), U256::from(7))]);
        let mut cursor = HashedPostStateCursor::new_storage(
            mock_cursor,
            [&higher_priority, &exact_hit, &lower_priority],
            B256::ZERO,
        );

        let result = cursor.seek(key(0x05)).unwrap();
        assert_eq!(result, Some((key(0x05), U256::from(5))));
        assert_eq!(cursor.post_state_cursor.cursors[0].idx, 1);
        assert_eq!(cursor.post_state_cursor.cursors[1].idx, 0);
        assert_eq!(
            cursor.post_state_cursor.cursors[2].idx, 0,
            "lower-priority overlay should not be sought after an exact overlay hit"
        );
        assert!(visited_keys.lock().is_empty(), "exact overlay hit should not touch the DB cursor");

        let result = cursor.next().unwrap();
        assert_eq!(result, Some((key(0x06), U256::from(6))));
        assert!(!visited_keys.lock().is_empty(), "next should lazily position the DB cursor");
    }

    #[test]
    fn test_seek_can_move_backwards() {
        let db_nodes = BTreeMap::from([(key(0x01), U256::from(1)), (key(0x03), U256::from(3))]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);

        let post_state = storage_post_state(vec![(key(0x02), U256::from(2))]);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, [&post_state], B256::ZERO);

        assert_eq!(cursor.seek(key(0x03)).unwrap(), Some((key(0x03), U256::from(3))));
        assert_eq!(cursor.seek(key(0x01)).unwrap(), Some((key(0x01), U256::from(1))));
        assert_eq!(cursor.next().unwrap(), Some((key(0x02), U256::from(2))));
    }

    #[test]
    fn test_multiple_overlays_resolve_by_precedence() {
        let db_nodes = BTreeMap::from([
            (key(0x01), U256::from(1)),
            (key(0x02), U256::from(2)),
            (key(0x04), U256::from(4)),
        ]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);

        let newest = storage_post_state(vec![(key(0x02), U256::ZERO), (key(0x03), U256::from(30))]);
        let oldest = storage_post_state(vec![
            (key(0x01), U256::from(10)),
            (key(0x02), U256::from(20)),
            (key(0x03), U256::from(3)),
        ]);
        let mut cursor =
            HashedPostStateCursor::new_storage(mock_cursor, [&newest, &oldest], B256::ZERO);

        let mut results = Vec::new();
        if let Some(entry) = cursor.seek(B256::ZERO).unwrap() {
            results.push(entry);
            while let Some(entry) = cursor.next().unwrap() {
                results.push(entry);
            }
        }

        assert_eq!(
            results,
            vec![
                (key(0x01), U256::from(10)),
                (key(0x03), U256::from(30)),
                (key(0x04), U256::from(4)),
            ]
        );
    }

    #[test]
    fn test_indexed_account_overlay_resolves_by_precedence() {
        let db_nodes = BTreeMap::from([(key(0x01), account(1)), (key(0x03), account(3))]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);

        let newest = HashedPostStateSorted::new(
            vec![(key(0x01), None), (key(0x02), Some(account(20)))],
            Default::default(),
        );
        let oldest = HashedPostStateSorted::new(
            vec![(key(0x01), Some(account(10))), (key(0x03), Some(account(30)))],
            Default::default(),
        );
        let overlay = HashedPostStateOverlay::new(vec![Arc::new(newest), Arc::new(oldest)]);
        let mut cursor = HashedPostStateCursor::new_account_from_overlay(mock_cursor, &overlay);

        let mut results = Vec::new();
        if let Some(entry) = cursor.seek(B256::ZERO).unwrap() {
            results.push(entry);
            while let Some(entry) = cursor.next().unwrap() {
                results.push(entry);
            }
        }

        assert_eq!(results, vec![(key(0x02), account(20)), (key(0x03), account(30))]);
    }

    #[test]
    fn test_storage_wipe_overlay_hides_lower_precedence_sources() {
        let db_nodes = BTreeMap::from([(key(0x04), U256::from(4))]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);

        let newest = storage_post_state(vec![(key(0x02), U256::from(2))]);
        let wiping = storage_post_state_with_wipe(vec![(key(0x01), U256::from(1))], true);
        let hidden = storage_post_state(vec![(key(0x03), U256::from(3))]);
        let mut cursor = HashedPostStateCursor::new_storage(
            mock_cursor,
            [&newest, &wiping, &hidden],
            B256::ZERO,
        );

        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((key(0x01), U256::from(1))));
        assert_eq!(cursor.next().unwrap(), Some((key(0x02), U256::from(2))));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn test_indexed_storage_wipe_overlay_hides_lower_precedence_sources() {
        let db_nodes = BTreeMap::from([(key(0x04), U256::from(4))]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);

        let newest = storage_post_state(vec![(key(0x02), U256::from(2))]);
        let wiping = storage_post_state_with_wipe(vec![(key(0x01), U256::from(1))], true);
        let hidden = storage_post_state(vec![(key(0x03), U256::from(3))]);
        let overlay =
            HashedPostStateOverlay::new(vec![Arc::new(newest), Arc::new(wiping), Arc::new(hidden)]);
        let mut cursor =
            HashedPostStateCursor::new_storage_from_overlay(mock_cursor, &overlay, B256::ZERO);

        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((key(0x01), U256::from(1))));
        assert_eq!(cursor.next().unwrap(), Some((key(0x02), U256::from(2))));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn test_indexed_storage_overlay_switches_hashed_address() {
        let first_address = B256::with_last_byte(1);
        let second_address = B256::with_last_byte(2);
        let mut db_storage = B256Map::default();
        db_storage.insert(first_address, BTreeMap::from([(key(0x04), U256::from(4))]));
        db_storage.insert(second_address, BTreeMap::from([(key(0x05), U256::from(5))]));
        let visited_keys =
            Arc::new(db_storage.keys().map(|key| (*key, Default::default())).collect());
        let mock_cursor =
            MockHashedCursor::new_storage(Arc::new(db_storage), visited_keys, first_address)
                .unwrap();

        let first_overlay =
            storage_post_state_for_address(first_address, vec![(key(0x01), U256::from(1))]);
        let second_overlay =
            storage_post_state_for_address(second_address, vec![(key(0x02), U256::from(2))]);
        let overlay =
            HashedPostStateOverlay::new(vec![Arc::new(first_overlay), Arc::new(second_overlay)]);
        let mut cursor =
            HashedPostStateCursor::new_storage_from_overlay(mock_cursor, &overlay, first_address);

        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((key(0x01), U256::from(1))));
        assert_eq!(cursor.next().unwrap(), Some((key(0x04), U256::from(4))));

        cursor.set_hashed_address(second_address);

        assert_eq!(cursor.seek(B256::ZERO).unwrap(), Some((key(0x02), U256::from(2))));
        assert_eq!(cursor.next().unwrap(), Some((key(0x05), U256::from(5))));
        assert_eq!(cursor.next().unwrap(), None);
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        #[derive(Clone, Debug)]
        enum CursorOp {
            Next,
            Seek(B256),
        }

        /// Merge `db_nodes` with post-state overlays. Overlay index 0 has highest priority.
        fn merge_with_overlays(
            db_nodes: &[(B256, U256)],
            overlays: &[Vec<(B256, U256)>],
        ) -> Vec<(B256, U256)> {
            let mut merged: BTreeMap<B256, U256> = db_nodes.iter().copied().collect();

            for overlay in overlays.iter().rev() {
                for (key, value) in overlay {
                    if value.is_zero() {
                        merged.remove(key);
                    } else {
                        merged.insert(*key, *value);
                    }
                }
            }

            merged.into_iter().collect()
        }

        fn reference_seek(
            entries: &[(B256, U256)],
            position: &mut Option<usize>,
            key: B256,
        ) -> Option<(B256, U256)> {
            let idx = entries.partition_point(|(entry_key, _)| entry_key < &key);
            if idx < entries.len() {
                *position = Some(idx);
                Some(entries[idx])
            } else {
                *position = None;
                None
            }
        }

        fn reference_next(
            entries: &[(B256, U256)],
            position: &mut Option<usize>,
        ) -> Option<(B256, U256)> {
            let next_idx = position.and_then(|idx| idx.checked_add(1))?;

            if next_idx < entries.len() {
                *position = Some(next_idx);
                Some(entries[next_idx])
            } else {
                *position = None;
                None
            }
        }

        fn nonzero_u256_strategy() -> impl Strategy<Value = U256> {
            any::<u64>().prop_map(|value| U256::from(value.saturating_add(1)))
        }

        /// Generate a sorted vector of (B256, U256) entries
        fn sorted_db_nodes_strategy() -> impl Strategy<Value = Vec<(B256, U256)>> {
            prop::collection::vec((any::<u8>(), nonzero_u256_strategy()), 0..20).prop_map(
                |entries| {
                    let mut result: Vec<(B256, U256)> = entries
                        .into_iter()
                        .map(|(byte, value)| (B256::repeat_byte(byte), value))
                        .collect();
                    result.sort_by_key(|a| a.0);
                    result.dedup_by(|a, b| a.0 == b.0);
                    result
                },
            )
        }

        /// Generate a sorted vector of (B256, U256) entries (including deletions as ZERO)
        fn sorted_post_state_nodes_strategy() -> impl Strategy<Value = Vec<(B256, U256)>> {
            // Explicitly inject ZERO values to model post-state deletions.
            prop::collection::vec((any::<u8>(), nonzero_u256_strategy(), any::<bool>()), 0..20)
                .prop_map(|entries| {
                    let mut result: Vec<(B256, U256)> = entries
                        .into_iter()
                        .map(|(byte, value, is_deletion)| {
                            let effective_value = if is_deletion { U256::ZERO } else { value };
                            (B256::repeat_byte(byte), effective_value)
                        })
                        .collect();
                    result.sort_by_key(|a| a.0);
                    result.dedup_by(|a, b| a.0 == b.0);
                    result
                })
        }

        fn cursor_ops_strategy() -> impl Strategy<Value = Vec<CursorOp>> {
            prop::collection::vec(
                prop_oneof![
                    Just(CursorOp::Next),
                    any::<u8>().prop_map(|byte| CursorOp::Seek(B256::repeat_byte(byte))),
                ],
                10..500,
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(10000))]

            /// Tests `HashedPostStateCursor` against a pre-merged reference cursor.
            #[test]
            fn proptest_hashed_post_state_cursor(
                db_nodes in sorted_db_nodes_strategy(),
                overlays in prop::collection::vec(sorted_post_state_nodes_strategy(), 0..5),
                ops in cursor_ops_strategy(),
            ) {
                reth_tracing::init_test_tracing();
                use tracing::debug;

                debug!(
                    db_keys=?db_nodes.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                    overlays=?overlays
                        .iter()
                        .map(|overlay| overlay.iter().map(|(k, v)| (k, !v.is_zero())).collect::<Vec<_>>())
                        .collect::<Vec<_>>(),
                    num_ops=?ops.len(),
                    "Starting proptest!",
                );

                let expected_combined = merge_with_overlays(&db_nodes, &overlays);
                let mut reference_position = None;

                // Create the HashedPostStateCursor being tested
                let db_nodes_map: BTreeMap<B256, U256> = db_nodes.iter().copied().collect();
                let db_nodes_arc = Arc::new(db_nodes_map);
                let visited_keys = Arc::new(Mutex::new(Vec::new()));
                let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);

                let hashed_address = B256::ZERO;
                let post_states =
                    overlays.into_iter().map(storage_post_state).collect::<Vec<_>>();
                let mut test_cursor =
                    HashedPostStateCursor::new_storage(mock_cursor, post_states.iter(), hashed_address);

                // Test: seek to the beginning first
                let control_first =
                    reference_seek(&expected_combined, &mut reference_position, B256::ZERO);
                let test_first = test_cursor.seek(B256::ZERO).unwrap();
                debug!(
                    control=?control_first.as_ref().map(|(k, _)| k),
                    test=?test_first.as_ref().map(|(k, _)| k),
                    "Initial seek returned",
                );
                assert_eq!(control_first, test_first, "Initial seek mismatch");

                // Execute a sequence of random operations
                for op in ops {
                    match op {
                        CursorOp::Next => {
                            let control_result =
                                reference_next(&expected_combined, &mut reference_position);
                            let test_result = test_cursor.next().unwrap();
                            debug!(
                                control=?control_result.as_ref().map(|(k, _)| k),
                                test=?test_result.as_ref().map(|(k, _)| k),
                                "Next returned",
                            );
                            assert_eq!(control_result, test_result, "Next operation mismatch");
                        }
                        CursorOp::Seek(key) => {
                            let control_result =
                                reference_seek(&expected_combined, &mut reference_position, key);
                            let test_result = test_cursor.seek(key).unwrap();
                            debug!(
                                control=?control_result.as_ref().map(|(k, _)| k),
                                test=?test_result.as_ref().map(|(k, _)| k),
                                ?key,
                                "Seek returned",
                            );
                            assert_eq!(control_result, test_result, "Seek operation mismatch for key {:?}", key);
                        }
                    }
                }
            }
        }
    }
}
