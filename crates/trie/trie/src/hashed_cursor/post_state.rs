use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use crate::forward_cursor::ForwardInMemoryCursor;
use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::HashedPostStateSorted;

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
        Ok(HashedPostStateCursor::new(Some(cursor), &self.post_state.as_ref().accounts))
    }

    /// Returns a `HashedPostStateCursor` for storage slots that merges:
    /// 1. `post_state_cursor` - In-memory overlay of storage updates (None if address has no
    ///    updates)
    /// 2. `cursor` - DB cursor for existing storage (None if storage was wiped via `SELF-DESTRUCT`)
    ///
    /// When storage is wiped, the DB cursor is omitted since all previous storage is destroyed.
    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        static EMPTY_UPDATES: Vec<(B256, U256)> = Vec::new();

        let post_state_storage = self.post_state.as_ref().storages.get(&hashed_address);
        let (storage_slots, wiped) = post_state_storage
            .map(|u| (u.storage_slots_ref(), u.is_wiped()))
            .unwrap_or((&EMPTY_UPDATES, false));

        let cursor = if wiped {
            None
        } else {
            Some(self.cursor_factory.hashed_storage_cursor(hashed_address)?)
        };

        Ok(HashedPostStateCursor::new(cursor, storage_slots))
    }
}

/// Trait for wrapper types that can be converted to `Option<NonZero>`.
///
/// This enables uniform handling of deletions across different wrapper types:
/// - `Option<Account>`: `None` indicates deletion
/// - `U256`: `U256::ZERO` indicates deletion (maps to `None`)
///
/// This design avoids the memory overhead of `Option<U256>` while maintaining
/// uniform handling of deletions across different value types.
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
        (self != Self::ZERO).then_some(self)
    }
}

/// A cursor to iterate over state updates and corresponding database entries.
/// It will always give precedence to the data from the post state updates.
#[derive(Debug)]
pub struct HashedPostStateCursor<'a, C, V>
where
    V: HashedPostStateCursorValue,
{
    /// The underlying `database_cursor`. If None then it is assumed there is no DB data.
    cursor: Option<C>,
    /// Entry that `database_cursor` is currently pointing to.
    cursor_entry: Option<(B256, V::NonZero)>,
    /// Forward-only in-memory cursor over underlying V.
    post_state_cursor: ForwardInMemoryCursor<'a, B256, V>,
    /// The last hashed key that was returned by the cursor.
    /// De facto, this is a current cursor position.
    last_key: Option<B256>,
    /// Tracks whether `seek` has been called. Used to prevent re-seeking the DB cursor
    /// when it has been exhausted by iteration.
    seeked: bool,
}

impl<'a, C, V> HashedPostStateCursor<'a, C, V>
where
    C: HashedCursor<Value = V::NonZero>,
    V: HashedPostStateCursorValue,
{
    /// Creates a new post state cursor which combines a DB cursor and in-memory post state updates.
    ///
    /// # Parameters
    /// - `cursor`: The database cursor. Pass `None` to indicate:
    ///   - For accounts: Empty database (no persisted accounts)
    ///   - For storage: Wiped storage (e.g., via `SELFDESTRUCT` - all previous storage destroyed)
    /// - `updates`: Pre-sorted post state updates where `Some(value)` indicates an update and
    ///   `None` indicates a deletion (destroyed account or zero-valued storage slot)
    pub fn new(cursor: Option<C>, updates: &'a [(B256, V)]) -> Self {
        debug_assert!(updates.is_sorted_by_key(|(k, _)| k), "Overlay values must be sorted by key");
        Self {
            cursor,
            cursor_entry: None,
            post_state_cursor: ForwardInMemoryCursor::new(updates),
            last_key: None,
            seeked: false,
        }
    }

    /// Asserts that the next entry to be returned from the cursor is not previous to the last entry
    /// returned.
    fn set_last_key(&mut self, next_entry: &Option<(B256, V::NonZero)>) {
        self.last_key = next_entry.as_ref().map(|e| e.0);
    }

    /// Seeks the `cursor_entry` field of the struct using the cursor.
    fn cursor_seek(&mut self, key: B256) -> Result<(), DatabaseError> {
        // Only seek if:
        // 1. We have a cursor entry and need to seek forward (entry.0 < key), OR
        // 2. We have no cursor entry and haven't seeked yet (!self.seeked)
        let should_seek = match self.cursor_entry.as_ref() {
            Some(entry) => entry.0 < key,
            None => !self.seeked,
        };

        if should_seek {
            self.cursor_entry = self.cursor.as_mut().map(|c| c.seek(key)).transpose()?.flatten();
        }

        Ok(())
    }

    /// Seeks the `cursor_entry` field of the struct to the subsequent entry using the cursor.
    fn cursor_next(&mut self) -> Result<(), DatabaseError> {
        // If the previous entry is `None`, and we've done a seek previously, then the cursor is
        // exhausted, and we shouldn't call `next` again.
        if self.cursor_entry.is_some() {
            self.cursor_entry = self.cursor.as_mut().map(|c| c.next()).transpose()?.flatten();
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
                self.post_state_cursor.current().copied().map(|(k, v)| (k, v.into_option()));

            match (post_state_current, &self.cursor_entry) {
                (Some((mem_key, None)), _)
                    if self.cursor_entry.as_ref().is_none_or(|(db_key, _)| &mem_key < db_key) =>
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
                    if self.cursor_entry.as_ref().is_none_or(|(db_key, _)| &mem_key <= db_key) =>
                {
                    // If overlay returns a value prior to the DB's value, or the DB is exhausted,
                    // then we return the overlay's value.
                    return Ok(Some((mem_key, value)))
                }
                // All other cases:
                // - mem_key > db_key
                // - overlay is exhausted
                // Return the db_entry. If DB is also exhausted then this returns None.
                _ => return Ok(self.cursor_entry),
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
        self.cursor_seek(key)?;
        self.post_state_cursor.seek(&key);

        let entry = self.choose_next_entry()?;
        self.set_last_key(&entry);
        self.seeked = true;
        Ok(entry)
    }

    /// Retrieve the next entry from the cursor.
    ///
    /// If the cursor is positioned at the entry, return the entry with next greater key.
    /// Returns [None] if the previous memoized or the next greater entries are missing.
    ///
    /// NOTE: This function will not return any entry unless [`HashedCursor::seek`] has been called.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        // A `last_key` of `None` indicates that the cursor is exhausted.
        let Some(last_key) = self.last_key else {
            return Ok(None);
        };

        // If either cursor is currently pointing to the last entry which was returned then consume
        // that entry so that `choose_next_entry` is looking at the subsequent one.
        if let Some((key, _)) = self.post_state_cursor.current() &&
            key == &last_key
        {
            self.post_state_cursor.first_after(&last_key);
        }

        if let Some((key, _)) = &self.cursor_entry &&
            key == &last_key
        {
            self.cursor_next()?;
        }

        let entry = self.choose_next_entry()?;
        self.set_last_key(&entry);
        Ok(entry)
    }
}

/// The cursor to iterate over post state hashed values and corresponding database entries.
/// It will always give precedence to the data from the post state.
impl<C, V> HashedStorageCursor for HashedPostStateCursor<'_, C, V>
where
    C: HashedStorageCursor<Value = V::NonZero>,
    V: HashedPostStateCursorValue,
{
    /// Returns `true` if the account has no storage entries.
    ///
    /// This function should be called before attempting to call [`HashedCursor::seek`] or
    /// [`HashedCursor::next`].
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        // Storage is not empty if it has non-zero slots.
        if self.post_state_cursor.has_any(|(_, value)| value.into_option().is_some()) {
            return Ok(false);
        }

        // If no non-zero slots in post state, check the database.
        // Returns true if cursor is None (wiped storage or empty DB).
        self.cursor.as_mut().map_or(Ok(true), |c| c.is_storage_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashed_cursor::mock::MockHashedCursor;
    use parking_lot::Mutex;
    use std::{collections::BTreeMap, sync::Arc};

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
            V::NonZero: Copy,
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
                result.sort_by(|a, b| a.0.cmp(&b.0));
                result.dedup_by(|a, b| a.0 == b.0);
                result
            })
        }

        /// Generate a sorted vector of (B256, U256) entries (including deletions as ZERO)
        fn sorted_post_state_nodes_strategy() -> impl Strategy<Value = Vec<(B256, U256)>> {
            prop::collection::vec((any::<u8>(), u256_strategy()), 0..20).prop_map(|entries| {
                let mut result: Vec<(B256, U256)> = entries
                    .into_iter()
                    .map(|(byte, value)| (B256::repeat_byte(byte), value))
                    .collect();
                result.sort_by(|a, b| a.0.cmp(&b.0));
                result.dedup_by(|a, b| a.0 == b.0);
                result
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1000))]

            #[test]
            fn proptest_hashed_post_state_cursor(
                db_nodes in sorted_db_nodes_strategy(),
                post_state_nodes in sorted_post_state_nodes_strategy(),
                op_choices in prop::collection::vec(any::<u8>(), 10..500),
            ) {
                reth_tracing::init_test_tracing();
                use tracing::debug;

                debug!("Starting proptest!");

                // Create the expected results by merging the two sorted vectors,
                // properly handling deletions (ZERO values in post_state_nodes)
                let expected_combined = merge_with_overlay(db_nodes.clone(), post_state_nodes.clone());

                // Collect all keys for operation generation
                let all_keys: Vec<B256> = expected_combined.iter().map(|(k, _)| *k).collect();

                // Create a control cursor using the combined result with a mock cursor
                let control_db_map: BTreeMap<B256, U256> = expected_combined.into_iter().collect();
                let control_db_arc = Arc::new(control_db_map);
                let control_visited_keys = Arc::new(Mutex::new(Vec::new()));
                let mut control_cursor = MockHashedCursor::new(control_db_arc, control_visited_keys);

                // Create the HashedPostStateCursor being tested
                let db_nodes_map: BTreeMap<B256, U256> = db_nodes.into_iter().collect();
                let db_nodes_arc = Arc::new(db_nodes_map);
                let visited_keys = Arc::new(Mutex::new(Vec::new()));
                let mock_cursor = MockHashedCursor::new(db_nodes_arc, visited_keys);
                let mut test_cursor = HashedPostStateCursor::new(Some(mock_cursor), &post_state_nodes);

                // Test: seek to the beginning first
                let control_first = control_cursor.seek(B256::ZERO).unwrap();
                let test_first = test_cursor.seek(B256::ZERO).unwrap();
                debug!(
                    control=?control_first.as_ref().map(|(k, _)| k),
                    test=?test_first.as_ref().map(|(k, _)| k),
                    "Initial seek returned",
                );
                assert_eq!(control_first, test_first, "Initial seek mismatch");

                // If both cursors returned None, nothing to test
                if control_first.is_none() && test_first.is_none() {
                    return Ok(());
                }

                // Track the last key returned from the cursor
                let mut last_returned_key = control_first.as_ref().map(|(k, _)| *k);

                // Execute a sequence of random operations
                for choice in op_choices {
                    let op_type = choice % 2; // Only 2 operation types: next and seek

                    match op_type {
                        0 => {
                            // Next operation
                            let control_result = control_cursor.next().unwrap();
                            let test_result = test_cursor.next().unwrap();
                            debug!(
                                control=?control_result.as_ref().map(|(k, _)| k),
                                test=?test_result.as_ref().map(|(k, _)| k),
                                "Next returned",
                            );
                            assert_eq!(control_result, test_result, "Next operation mismatch");

                            last_returned_key = control_result.as_ref().map(|(k, _)| *k);

                            // Stop if both cursors are exhausted
                            if control_result.is_none() && test_result.is_none() {
                                break;
                            }
                        }
                        _ => {
                            // Seek operation - choose a key >= last_returned_key
                            if all_keys.is_empty() {
                                continue;
                            }

                            let valid_keys: Vec<_> = all_keys
                                .iter()
                                .filter(|k| last_returned_key.is_none_or(|last| **k >= last))
                                .collect();

                            if valid_keys.is_empty() {
                                continue;
                            }

                            let key = *valid_keys[(choice as usize / 2) % valid_keys.len()];

                            let control_result = control_cursor.seek(key).unwrap();
                            let test_result = test_cursor.seek(key).unwrap();
                            debug!(
                                control=?control_result.as_ref().map(|(k, _)| k),
                                test=?test_result.as_ref().map(|(k, _)| k),
                                ?key,
                                "Seek returned",
                            );
                            assert_eq!(control_result, test_result, "Seek operation mismatch for key {:?}", key);

                            last_returned_key = control_result.as_ref().map(|(k, _)| *k);

                            // Stop if both cursors are exhausted
                            if control_result.is_none() && test_result.is_none() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}
