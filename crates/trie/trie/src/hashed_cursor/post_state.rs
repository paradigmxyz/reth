use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use crate::{cursor_position::CursorPosition, forward_cursor::ForwardInMemoryCursor};
use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostStateSorted;
use std::ops::Bound;

/// The hashed cursor factory for the post state.
#[derive(Clone, Debug)]
pub struct HashedPostStateCursorFactory<CF, T> {
    cursor_factory: CF,
    post_state: T,
}

impl<CF, T> HashedPostStateCursorFactory<CF, T> {
    /// Create a new factory.
    pub const fn new(cursor_factory: CF, post_state: T) -> Self {
        Self { cursor_factory, post_state }
    }
}

impl<'overlay, CF, T> HashedCursorFactory for HashedPostStateCursorFactory<CF, &'overlay T>
where
    CF: HashedCursorFactory,
    T: AsRef<HashedPostStateSorted>,
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
        Ok(HashedPostStateCursor::new_account(cursor, self.post_state.as_ref()))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let post_state = self.post_state.as_ref();
        let cursor = self.cursor_factory.hashed_storage_cursor(hashed_address)?;
        Ok(HashedPostStateCursor::new_storage(cursor, post_state, hashed_address))
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
pub trait HashedPostStateCursorValue: Clone {
    /// The non-zero type returned by `into_option`.
    /// For `Option<Account>`, this is `Account`.
    /// For `U256`, this is `U256`.
    type NonZero: Clone + std::fmt::Debug;

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

/// A cursor to iterate over state updates and corresponding database entries.
/// It will always give precedence to the data from the post state updates.
#[derive(Debug)]
pub struct HashedPostStateCursor<'a, C, V>
where
    V: HashedPostStateCursorValue,
{
    /// The underlying cursor.
    cursor: C,
    /// Tracks whether the DB cursor is available, positioned, or exhausted.
    db_cursor_state: DbCursorState<V::NonZero>,
    /// Forward-only in-memory cursor over underlying V.
    post_state_cursor: ForwardInMemoryCursor<'a, B256, V>,
    /// The logical result and the request range covered by the cursor positions.
    position: CursorPosition<B256>,
    /// Reference to the full post state.
    post_state: &'a HashedPostStateSorted,
}

#[derive(Debug)]
enum DbCursorState<V> {
    NeedsPosition,
    Positioned((B256, V)),
    Exhausted,
}

impl<V> DbCursorState<V> {
    const fn entry(&self) -> Option<&(B256, V)> {
        match self {
            Self::Positioned(entry) => Some(entry),
            Self::NeedsPosition | Self::Exhausted => None,
        }
    }

    fn set_entry(&mut self, entry: Option<(B256, V)>) {
        *self = match entry {
            Some(entry) => Self::Positioned(entry),
            None => Self::Exhausted,
        };
    }
}

impl<'a, C> HashedPostStateCursor<'a, C, Option<Account>>
where
    C: HashedCursor<Value = Account>,
{
    /// Create new account cursor which combines a DB cursor and the post state.
    pub fn new_account(cursor: C, post_state: &'a HashedPostStateSorted) -> Self {
        let post_state_cursor = ForwardInMemoryCursor::new(&post_state.accounts);
        Self {
            cursor,
            db_cursor_state: DbCursorState::NeedsPosition,
            post_state_cursor,
            position: CursorPosition::Unseeked,
            post_state,
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
        post_state: &'a HashedPostStateSorted,
        hashed_address: B256,
    ) -> Self {
        let post_state_cursor = Self::get_storage_overlay(post_state, hashed_address);
        Self {
            cursor,
            db_cursor_state: DbCursorState::NeedsPosition,
            post_state_cursor,
            position: CursorPosition::Unseeked,
            post_state,
        }
    }

    /// Returns the storage overlay for `hashed_address`.
    fn get_storage_overlay(
        post_state: &'a HashedPostStateSorted,
        hashed_address: B256,
    ) -> ForwardInMemoryCursor<'a, B256, U256> {
        let post_state_storage = post_state.storages.get(&hashed_address);
        let storage_slots = post_state_storage.map(|u| u.storage_slots_ref()).unwrap_or(&[]);

        ForwardInMemoryCursor::new(storage_slots)
    }
}

impl<'a, C, V> HashedPostStateCursor<'a, C, V>
where
    C: HashedCursor<Value = V::NonZero>,
    V: HashedPostStateCursorValue,
{
    const fn get_cursor_mut(&mut self) -> &mut C {
        &mut self.cursor
    }

    /// Positions the DB cursor state using the underlying cursor when needed.
    /// Errors invalidate both cursor positions so a later seek can safely retry.
    fn cursor_seek(&mut self, key: B256) -> Result<(), DatabaseError> {
        // Only seek if:
        // 1. We have a cursor entry and need to seek forward (entry.0 < key), OR
        // 2. The DB cursor needs to be positioned.
        let should_seek = match &self.db_cursor_state {
            DbCursorState::NeedsPosition => true,
            DbCursorState::Positioned((entry_key, _)) => entry_key < &key,
            DbCursorState::Exhausted => false,
        };

        if should_seek {
            let entry = self.get_cursor_mut().seek(key).inspect_err(|_| self.reset())?;
            self.db_cursor_state.set_entry(entry);
        }

        Ok(())
    }

    /// Advances the DB cursor state to the subsequent entry using the underlying cursor.
    /// Errors invalidate both cursor positions so a later seek can safely retry.
    fn cursor_next(&mut self) -> Result<(), DatabaseError> {
        debug_assert!(!matches!(self.position, CursorPosition::Unseeked));

        // Exhausted DB state is stable; only advance when the DB cursor is positioned at an entry.
        if matches!(self.db_cursor_state, DbCursorState::Positioned(_)) {
            let entry = self.get_cursor_mut().next().inspect_err(|_| self.reset())?;
            self.db_cursor_state.set_entry(entry);
        }

        Ok(())
    }

    /// Compares the current in-memory entry with the current entry of the cursor, and applies the
    /// in-memory entry to the cursor entry as an overlay.
    ///
    /// This may consume and move forward the current entries when the overlay indicates a removed
    /// node.
    fn choose_next_entry(&mut self) -> Result<Option<(B256, V::NonZero)>, DatabaseError> {
        loop {
            let post_state_current =
                self.post_state_cursor.current().cloned().map(|(k, v)| (k, v.into_option()));
            let db_entry = self.db_cursor_state.entry();

            match (post_state_current, db_entry) {
                (Some((mem_key, None)), _)
                    if db_entry.is_none_or(|(db_key, _)| &mem_key < db_key) =>
                {
                    // If overlay has a removed value but DB cursor is exhausted or ahead of the
                    // in-memory cursor then move ahead in-memory, as there might be further
                    // non-removed overlay values.
                    self.post_state_cursor.first_after(&mem_key);
                }
                (Some((mem_key, None)), Some((db_key, _))) if &mem_key == db_key => {
                    // If overlay has a removed value which is returned from DB then move both
                    // cursors ahead to the next key.
                    self.post_state_cursor.first_after(&mem_key);
                    self.cursor_next()?;
                }
                (Some((mem_key, Some(value))), _)
                    if db_entry.is_none_or(|(db_key, _)| &mem_key <= db_key) =>
                {
                    // If overlay returns a value prior to the DB's value, or the DB is exhausted,
                    // then we return the overlay's value.
                    return Ok(Some((mem_key, value)))
                }
                // All other cases:
                // - mem_key > db_key
                // - overlay is exhausted
                // Return the db_entry. If DB is also exhausted then this returns None.
                _ => return Ok(db_entry.cloned()),
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
        // A returned successor may be ahead of the request after scanning deleted entries.
        // Rewind only when the request precedes the range those scans established.
        if self.position.must_rewind(key) {
            self.reset();
        }
        self.position.set_lower_bound(Bound::Included(key));
        let post_state_entry =
            self.post_state_cursor.seek(&key).cloned().map(|(k, v)| (k, v.into_option()));

        if let Some((mem_key, Some(value))) = post_state_entry &&
            mem_key == key
        {
            // An exact overlay hit is the first logical entry at or after `key`, so the DB cursor
            // can stay lazy until a later operation needs it.
            if matches!(&self.db_cursor_state, DbCursorState::Positioned((db_key, _)) if db_key < &key)
            {
                self.db_cursor_state = DbCursorState::NeedsPosition;
            }

            let entry = Some((key, value));
            self.position.set_key(entry.as_ref().map(|entry| entry.0));
            return Ok(entry)
        }

        self.cursor_seek(key)?;

        let entry = self.choose_next_entry()?;
        self.position.set_key(entry.as_ref().map(|entry| entry.0));
        Ok(entry)
    }

    /// Retrieve the next entry from the cursor.
    ///
    /// If the cursor is positioned at the entry, return the entry with next greater key.
    /// Returns [None] if the previous memoized or the next greater entries are missing.
    ///
    /// NOTE: This function will not return any entry unless [`HashedCursor::seek`] has been called.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        debug_assert!(
            !matches!(self.position, CursorPosition::Unseeked),
            "Cursor must be seek'd before next is called",
        );

        // A missing logical result leaves the reusable request bound unchanged.
        let Some(last_key) = self.position.key() else {
            return Ok(None);
        };

        // Consuming the current entry makes it unavailable to subsequent seeks at or below it.
        self.position.set_lower_bound(Bound::Excluded(last_key));

        // If either cursor is currently pointing to the last entry which was returned then consume
        // that entry so that `choose_next_entry` is looking at the subsequent one.
        if let Some((key, _)) = self.post_state_cursor.current() &&
            key == &last_key
        {
            self.post_state_cursor.first_after(&last_key);
        }

        if matches!(self.db_cursor_state, DbCursorState::NeedsPosition) {
            self.cursor_seek(last_key)?;
        }

        if let Some((key, _)) = self.db_cursor_state.entry() &&
            key == &last_key
        {
            self.cursor_next()?;
        }

        let entry = self.choose_next_entry()?;
        self.position.set_key(entry.as_ref().map(|entry| entry.0));
        Ok(entry)
    }

    fn reset(&mut self) {
        let Self { cursor, db_cursor_state, post_state_cursor, position, .. } = self;

        cursor.reset();
        post_state_cursor.reset();

        *db_cursor_state = DbCursorState::NeedsPosition;
        *position = CursorPosition::Unseeked;
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
        let is_empty = self.seek(B256::ZERO)?.is_none();
        self.reset();
        Ok(is_empty)
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.reset();
        self.cursor.set_hashed_address(hashed_address);
        let post_state_cursor =
            HashedPostStateCursor::<C, U256>::get_storage_overlay(self.post_state, hashed_address);
        self.post_state_cursor = post_state_cursor;
        self.db_cursor_state = DbCursorState::NeedsPosition;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashed_cursor::mock::{MockHashedCursor, MockHashedCursorFactory};
    use alloy_primitives::map::B256Map;
    use reth_concurrency::sync::parking_lot::Mutex;
    use reth_trie_common::{HashedPostState, HashedStorage};
    use std::{collections::BTreeMap, sync::Arc};

    fn key(byte: u8) -> B256 {
        B256::repeat_byte(byte)
    }

    fn storage_post_state(storage_slots: Vec<(B256, U256)>) -> HashedPostStateSorted {
        let storage_sorted = reth_trie_common::HashedStorageSorted { storage_slots };
        let mut storages = alloy_primitives::map::B256Map::default();
        storages.insert(B256::ZERO, storage_sorted);
        HashedPostStateSorted::new(Vec::new(), storages)
    }

    #[reth_concurrency::test]
    fn test_seek_overlay_exact_hit_does_not_touch_db_until_next() {
        let db_nodes = vec![(key(0x02), U256::from(2)), (key(0x03), U256::from(3))];
        let post_state_nodes = vec![(key(0x02), U256::from(42))];

        let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let post_state = storage_post_state(post_state_nodes);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, &post_state, B256::ZERO);

        let result = cursor.seek(key(0x02)).unwrap();
        assert_eq!(result, Some((key(0x02), U256::from(42))));
        assert!(visited_keys.lock().is_empty(), "exact overlay hit should not touch the DB cursor");

        let result = cursor.next().unwrap();
        assert_eq!(result, Some((key(0x03), U256::from(3))));
        assert!(!visited_keys.lock().is_empty(), "next should lazily position the DB cursor");
    }

    #[reth_concurrency::test]
    fn test_seek_overlay_exact_hit_repositions_stale_db_on_next() {
        let db_nodes = vec![(key(0x01), U256::ONE), (key(0x03), U256::from(3))];
        let post_state_nodes = vec![(key(0x02), U256::from(2))];

        let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let post_state = storage_post_state(post_state_nodes);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, &post_state, B256::ZERO);

        let result = cursor.seek(key(0x01)).unwrap();
        assert_eq!(result, Some((key(0x01), U256::ONE)));
        assert_eq!(visited_keys.lock().len(), 1);

        let result = cursor.seek(key(0x02)).unwrap();
        assert_eq!(result, Some((key(0x02), U256::from(2))));
        assert_eq!(visited_keys.lock().len(), 1, "exact overlay hit should not seek the DB");

        let result = cursor.next().unwrap();
        assert_eq!(result, Some((key(0x03), U256::from(3))));
    }

    #[reth_concurrency::test]
    fn test_seek_overlay_exact_deletion_still_seeks_db() {
        let db_nodes = vec![(key(0x02), U256::from(2)), (key(0x03), U256::from(3))];
        let post_state_nodes = vec![(key(0x02), U256::ZERO)];

        let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys.clone());

        let post_state = storage_post_state(post_state_nodes);
        let mut cursor = HashedPostStateCursor::new_storage(mock_cursor, &post_state, B256::ZERO);

        let result = cursor.seek(key(0x02)).unwrap();
        assert_eq!(result, Some((key(0x03), U256::from(3))));
        assert!(!visited_keys.lock().is_empty(), "exact overlay deletion should consult the DB");
    }

    mod proptest_tests {
        use super::*;
        use itertools::Itertools;
        use proptest::prelude::*;

        /// Merge `db_nodes` with `post_state_nodes`, applying the post state overlay.
        /// This properly handles deletions (ZERO values for U256, None for Account).
        fn merge_with_overlay<V>(
            db_nodes: Vec<(B256, V::NonZero)>,
            post_state_nodes: Vec<(B256, V)>,
        ) -> Vec<(B256, V::NonZero)>
        where
            V: HashedPostStateCursorValue,
            V::NonZero: Clone,
        {
            db_nodes
                .into_iter()
                .merge_join_by(post_state_nodes, |db_entry, mem_entry| db_entry.0.cmp(&mem_entry.0))
                .filter_map(|entry| match entry {
                    // Only in db: keep it
                    itertools::EitherOrBoth::Left((key, node)) => Some((key, node)),
                    // Only in post state: keep if not a deletion
                    itertools::EitherOrBoth::Right((key, wrapped)) => {
                        wrapped.into_option().map(|val| (key, val))
                    }
                    // In both: post state takes precedence (keep if not a deletion)
                    itertools::EitherOrBoth::Both(_, (key, wrapped)) => {
                        wrapped.into_option().map(|val| (key, val))
                    }
                })
                .collect()
        }

        /// Generate a strategy for U256 values
        fn u256_strategy() -> impl Strategy<Value = U256> {
            any::<u64>().prop_map(U256::from)
        }

        /// Generate a sorted vector of (B256, U256) entries
        fn sorted_db_nodes_strategy() -> impl Strategy<Value = Vec<(B256, U256)>> {
            prop::collection::vec((any::<u8>(), u256_strategy()), 0..20).prop_map(|entries| {
                let mut result: Vec<(B256, U256)> = entries
                    .into_iter()
                    .map(|(byte, value)| (B256::repeat_byte(byte), value))
                    .collect();
                result.sort_by_key(|a| a.0);
                result.dedup_by(|a, b| a.0 == b.0);
                result
            })
        }

        /// Generate a sorted vector of (B256, U256) entries (including deletions as ZERO)
        fn sorted_post_state_nodes_strategy() -> impl Strategy<Value = Vec<(B256, U256)>> {
            // Explicitly inject ZERO values to model post-state deletions.
            prop::collection::vec((any::<u8>(), u256_strategy(), any::<bool>()), 0..20).prop_map(
                |entries| {
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
                },
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1000))]

            /// Compares arbitrary cursor operations with the merged state and checks that
            /// forward requests inside known gaps do not repeat database work.
            #[reth_concurrency::test]
            fn proptest_hashed_post_state_cursor(
                db_nodes in sorted_db_nodes_strategy(),
                post_state_nodes in sorted_post_state_nodes_strategy(),
                operations in prop::collection::vec((0u8..3, any::<u8>()), 10..500),
            ) {
                let merged = merge_with_overlay::<U256>(db_nodes.clone(), post_state_nodes.clone())
                    .into_iter().collect::<BTreeMap<_, _>>();
                // Include deleted and absent keys, not just surviving keys returned by the cursor.
                let seek_keys = db_nodes.iter().map(|(key, _)| *key)
                    .chain(post_state_nodes.iter().map(|(key, _)| *key))
                    .chain(operations.iter().map(|(_, byte)| B256::repeat_byte(*byte)))
                    .sorted().dedup().collect::<Vec<_>>();
                let visited = Arc::new(Mutex::new(Vec::new()));
                let db = MockHashedCursor::new(
                    Arc::new(db_nodes.into_iter().collect()), visited.clone(),
                );
                let post_state = storage_post_state(post_state_nodes);
                let mut cursor = HashedPostStateCursor::new_storage(db, &post_state, B256::ZERO);
                let mut previous: Option<Option<B256>> = None;
                for key in seek_keys {
                    let visits = visited.lock().len();
                    let actual = cursor.seek(key).unwrap();
                    let expected = merged.range(key..).next().map(|(key, value)| (*key, *value));
                    prop_assert_eq!(actual, expected);
                    if previous.is_some_and(|successor| successor.is_none_or(|next| key <= next)) {
                        prop_assert_eq!(visited.lock().len(), visits,
                            "forward seek inside a known gap repeated DB work at {:?}", key);
                    }
                    previous = Some(expected.map(|(key, _)| key));
                }

                cursor.reset();
                let mut expected = merged.first_key_value().map(|(key, value)| (*key, *value));
                prop_assert_eq!(cursor.seek(B256::ZERO).unwrap(), expected);
                for (operation, byte) in operations {
                    let actual = if operation == 0 {
                        expected = expected.and_then(|(key, _)| {
                            merged.range((Bound::Excluded(key), Bound::Unbounded))
                                .next().map(|(key, value)| (*key, *value))
                        });
                        cursor.next().unwrap()
                    } else {
                        if operation == 2 {
                            cursor.reset();
                        }
                        let key = B256::repeat_byte(byte);
                        expected = merged.range(key..).next().map(|(key, value)| (*key, *value));
                        cursor.seek(key).unwrap()
                    };
                    prop_assert_eq!(actual, expected);
                }
            }
        }
    }

    #[reth_concurrency::test]
    fn seeks_after_next_preserve_gaps_and_rewind_consumed_entries() {
        let visited = Arc::new(Mutex::new(Vec::new()));
        let db = MockHashedCursor::new(
            Arc::new([1, 2, 3, 4, 7].into_iter().map(|byte| (key(byte), U256::ONE)).collect()),
            visited.clone(),
        );
        let post_state = storage_post_state(vec![(key(2), U256::ZERO), (key(3), U256::ZERO)]);
        let mut cursor = HashedPostStateCursor::new_storage(db, &post_state, B256::ZERO);
        assert_eq!(cursor.seek(key(0)).unwrap(), Some((key(1), U256::ONE)));
        assert_eq!(cursor.next().unwrap(), Some((key(4), U256::ONE)));
        let operations = visited.lock().len();
        assert_eq!(cursor.seek(key(2)).unwrap(), Some((key(4), U256::ONE)));
        assert_eq!(visited.lock().len(), operations);
        assert_eq!(cursor.seek(key(1)).unwrap(), Some((key(1), U256::ONE)));
        assert_eq!(cursor.seek(key(4)).unwrap(), Some((key(4), U256::ONE)));
        assert_eq!(cursor.next().unwrap(), Some((key(7), U256::ONE)));
        assert_eq!(cursor.seek(key(5)).unwrap(), Some((key(7), U256::ONE)));
        assert_eq!(cursor.next().unwrap(), None);
        let operations = visited.lock().len();
        assert_eq!(cursor.next().unwrap(), None);
        assert_eq!(cursor.seek(key(8)).unwrap(), None);
        assert_eq!(visited.lock().len(), operations);
        assert_eq!(cursor.seek(key(7)).unwrap(), Some((key(7), U256::ONE)));
    }

    #[reth_concurrency::test]
    fn storage_address_switch_resets_seek_range() {
        let first = B256::ZERO;
        let second = key(1);
        let db = MockHashedCursorFactory::new(
            BTreeMap::new(),
            B256Map::from_iter([
                (first, BTreeMap::from([(key(8), U256::ONE)])),
                (second, BTreeMap::from([(key(1), U256::ONE), (key(2), U256::ONE)])),
            ]),
        );
        let post_state = HashedPostState::default()
            .with_storages([
                (first, HashedStorage::from_iter([(key(8), U256::ZERO)])),
                (second, HashedStorage::from_iter([(key(1), U256::ZERO)])),
            ])
            .into_sorted();
        let mut cursor = HashedPostStateCursor::new_storage(
            db.hashed_storage_cursor(first).unwrap(),
            &post_state,
            first,
        );
        assert_eq!(cursor.seek(key(8)).unwrap(), None);
        cursor.set_hashed_address(second);
        assert_eq!(cursor.seek(key(0)).unwrap(), Some((key(2), U256::ONE)));
        cursor.set_hashed_address(first);
        assert_eq!(cursor.seek(key(0)).unwrap(), None);
    }

    #[derive(Debug)]
    struct FailingHashedCursor {
        cursor: MockHashedCursor<U256>,
        fail_seek: bool,
        fail_next: bool,
    }

    impl HashedCursor for FailingHashedCursor {
        type Value = U256;

        fn seek(&mut self, key: B256) -> Result<Option<(B256, U256)>, DatabaseError> {
            let entry = self.cursor.seek(key)?;
            if self.fail_seek {
                self.fail_seek = false;
                return Err(DatabaseError::Decode);
            }
            Ok(entry)
        }

        fn next(&mut self) -> Result<Option<(B256, U256)>, DatabaseError> {
            let entry = self.cursor.next()?;
            if self.fail_next {
                self.fail_next = false;
                return Err(DatabaseError::Decode);
            }
            Ok(entry)
        }

        fn reset(&mut self) {
            self.cursor.reset();
        }
    }

    impl HashedStorageCursor for FailingHashedCursor {
        fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
            self.cursor.is_storage_empty()
        }

        fn set_hashed_address(&mut self, hashed_address: B256) {
            self.cursor.set_hashed_address(hashed_address);
        }
    }

    #[reth_concurrency::test]
    fn cursor_errors_invalidate_reusable_positions() {
        for operation in 0..3 {
            let db = MockHashedCursor::new(
                Arc::new([1, 2, 3].into_iter().map(|byte| (key(byte), U256::ONE)).collect()),
                Default::default(),
            );
            let post_state = storage_post_state(vec![(key(2), U256::ZERO)]);
            let mut cursor = HashedPostStateCursor::new_storage(
                FailingHashedCursor {
                    cursor: db,
                    fail_seek: operation == 0,
                    fail_next: operation != 0,
                },
                &post_state,
                B256::ZERO,
            );
            let error = match operation {
                0 => cursor.seek(key(1)),
                1 => {
                    assert_eq!(cursor.seek(key(1)).unwrap(), Some((key(1), U256::ONE)));
                    cursor.next()
                }
                _ => cursor.seek(key(2)),
            };
            assert!(matches!(error, Err(DatabaseError::Decode)));
            assert_eq!(cursor.position, CursorPosition::Unseeked);
            assert!(matches!(cursor.db_cursor_state, DbCursorState::NeedsPosition));
            assert_eq!(cursor.seek(key(2)).unwrap(), Some((key(3), U256::ONE)));
            assert_eq!(cursor.seek(key(1)).unwrap(), Some((key(1), U256::ONE)));
            assert_eq!(cursor.next().unwrap(), Some((key(3), U256::ONE)));
        }
    }
}
