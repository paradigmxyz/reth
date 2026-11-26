use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use crate::{
    cursor_position::CursorPosition, forward_cursor::ForwardInMemoryCursor,
    updates::TrieUpdatesSorted,
};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles};
use std::ops::Bound;

/// The trie cursor factory for the trie updates.
#[derive(Debug, Clone)]
pub struct InMemoryTrieCursorFactory<CF, T> {
    /// Underlying trie cursor factory.
    cursor_factory: CF,
    /// Reference to sorted trie updates.
    trie_updates: T,
}

impl<CF, T> InMemoryTrieCursorFactory<CF, T> {
    /// Create a new trie cursor factory.
    pub const fn new(cursor_factory: CF, trie_updates: T) -> Self {
        Self { cursor_factory, trie_updates }
    }
}

impl<'overlay, CF, T> TrieCursorFactory for InMemoryTrieCursorFactory<CF, &'overlay T>
where
    CF: TrieCursorFactory + 'overlay,
    T: AsRef<TrieUpdatesSorted>,
{
    type AccountTrieCursor<'cursor>
        = InMemoryTrieCursor<'overlay, CF::AccountTrieCursor<'cursor>>
    where
        Self: 'cursor;

    type StorageTrieCursor<'cursor>
        = InMemoryTrieCursor<'overlay, CF::StorageTrieCursor<'cursor>>
    where
        Self: 'cursor;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.account_trie_cursor()?;
        Ok(InMemoryTrieCursor::new_account(cursor, self.trie_updates.as_ref()))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let trie_updates = self.trie_updates.as_ref();
        let cursor = self.cursor_factory.storage_trie_cursor(hashed_address)?;
        Ok(InMemoryTrieCursor::new_storage(cursor, trie_updates, hashed_address))
    }
}

/// A cursor to iterate over trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
pub struct InMemoryTrieCursor<'a, C> {
    /// The underlying cursor.
    cursor: C,
    /// Tracks whether the DB cursor is available, positioned, or exhausted.
    db_cursor_state: DbCursorState,
    /// Forward-only in-memory cursor over storage trie nodes.
    in_memory_cursor: ForwardInMemoryCursor<'a, Nibbles, Option<BranchNodeCompact>>,
    /// The latest logical result and the lower bound of reusable seek requests.
    position: CursorPosition<Nibbles>,
    /// Reference to the full trie updates.
    trie_updates: &'a TrieUpdatesSorted,
}

#[derive(Debug)]
enum DbCursorState {
    NeedsPosition,
    Positioned((Nibbles, BranchNodeCompact)),
    Exhausted,
}

impl DbCursorState {
    const fn entry(&self) -> Option<&(Nibbles, BranchNodeCompact)> {
        match self {
            Self::Positioned(entry) => Some(entry),
            Self::NeedsPosition | Self::Exhausted => None,
        }
    }

    fn set_entry(&mut self, entry: Option<(Nibbles, BranchNodeCompact)>) {
        *self = match entry {
            Some(entry) => Self::Positioned(entry),
            None => Self::Exhausted,
        };
    }
}

impl<'a, C: TrieCursor> InMemoryTrieCursor<'a, C> {
    /// Create new account trie cursor which combines a DB cursor and the trie updates.
    pub fn new_account(cursor: C, trie_updates: &'a TrieUpdatesSorted) -> Self {
        let in_memory_cursor = ForwardInMemoryCursor::new(trie_updates.account_nodes_ref());
        Self {
            cursor,
            db_cursor_state: DbCursorState::NeedsPosition,
            in_memory_cursor,
            position: CursorPosition::Unseeked,
            trie_updates,
        }
    }

    /// Create new storage trie cursor with full trie updates reference.
    /// This allows the cursor to switch between storage tries when `set_hashed_address` is called.
    pub fn new_storage(
        cursor: C,
        trie_updates: &'a TrieUpdatesSorted,
        hashed_address: B256,
    ) -> Self {
        let in_memory_cursor = Self::get_storage_overlay(trie_updates, hashed_address);
        Self {
            cursor,
            db_cursor_state: DbCursorState::NeedsPosition,
            in_memory_cursor,
            position: CursorPosition::Unseeked,
            trie_updates,
        }
    }

    /// Returns the storage overlay for `hashed_address`.
    fn get_storage_overlay(
        trie_updates: &'a TrieUpdatesSorted,
        hashed_address: B256,
    ) -> ForwardInMemoryCursor<'a, Nibbles, Option<BranchNodeCompact>> {
        let storage_trie_updates = trie_updates.storage_tries_ref().get(&hashed_address);
        let storage_nodes = storage_trie_updates.map(|u| u.storage_nodes_ref()).unwrap_or(&[]);

        ForwardInMemoryCursor::new(storage_nodes)
    }

    const fn get_cursor_mut(&mut self) -> &mut C {
        &mut self.cursor
    }

    /// Positions the DB cursor state using the underlying cursor when needed.
    /// Errors invalidate both cursor positions so a later seek can safely retry.
    fn cursor_seek(&mut self, key: Nibbles) -> Result<(), DatabaseError> {
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
        debug_assert!(!matches!(self.db_cursor_state, DbCursorState::NeedsPosition));

        // The exhausted state is stable; only advance if the DB cursor currently points to an
        // entry.
        if matches!(self.db_cursor_state, DbCursorState::Positioned(_)) {
            let entry = self.get_cursor_mut().next().inspect_err(|_| self.reset())?;
            self.db_cursor_state.set_entry(entry);
        }

        Ok(())
    }

    /// Compares the current in-memory entry with the current entry of the cursor, and applies the
    /// in-memory entry to the cursor entry as an overlay.
    //
    /// This may consume and move forward the current entries when the overlay indicates a removed
    /// node.
    fn choose_next_entry(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        loop {
            let mem_entry = self.in_memory_cursor.current().cloned();
            let db_entry = self.db_cursor_state.entry();

            match (mem_entry, db_entry) {
                (Some((mem_key, None)), _)
                    if db_entry.is_none_or(|(db_key, _)| &mem_key < db_key) =>
                {
                    // If overlay has a removed node but DB cursor is exhausted or ahead of the
                    // in-memory cursor then move ahead in-memory, as there might be further
                    // non-removed overlay nodes.
                    self.in_memory_cursor.first_after(&mem_key);
                }
                (Some((mem_key, None)), Some((db_key, _))) if &mem_key == db_key => {
                    // If overlay has a removed node which is returned from DB then move both
                    // cursors ahead to the next key.
                    self.in_memory_cursor.first_after(&mem_key);
                    self.cursor_next()?;
                }
                (Some((mem_key, Some(node))), _)
                    if db_entry.is_none_or(|(db_key, _)| &mem_key <= db_key) =>
                {
                    // If overlay returns a node prior to the DB's node, or the DB is exhausted,
                    // then we return the overlay's node.
                    return Ok(Some((mem_key, node)))
                }
                // All other cases:
                // - mem_key > db_key
                // - overlay is exhausted
                // Return the db_entry. If DB is also exhausted then this returns None.
                _ => return Ok(db_entry.cloned()),
            }
        }
    }

    /// Rewinds only when a request precedes the range covered by the current positions.
    fn prepare_seek(&mut self, key: Nibbles) {
        if self.position.must_rewind(key) {
            self.reset();
        }
        // A returned successor may be ahead of the request after skipping deleted nodes.
        self.position.set_lower_bound(Bound::Included(key));
    }
}

impl<C: TrieCursor> TrieCursor for InMemoryTrieCursor<'_, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.prepare_seek(key);
        let mem_entry = self.in_memory_cursor.seek(&key);

        if let Some((mem_key, entry_inner)) = mem_entry &&
            *mem_key == key
        {
            // An exact overlay hit can move the logical cursor ahead without touching the DB. If
            // the DB cursor was still behind this key, force a re-seek before the next DB-backed
            // operation so `next()` cannot return a stale earlier entry.
            if matches!(&self.db_cursor_state, DbCursorState::Positioned((db_key, _)) if db_key < &key)
            {
                self.db_cursor_state = DbCursorState::NeedsPosition;
            }

            let entry = entry_inner.clone().map(|node| (key, node));
            self.position.set_key(entry.as_ref().map(|e| e.0));
            return Ok(entry)
        }

        self.cursor_seek(key)?;

        let entry = match self.db_cursor_state.entry() {
            Some((db_key, node)) if db_key == &key => Some((key, node.clone())),
            _ => None,
        };

        self.position.set_key(entry.as_ref().map(|e| e.0));
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.prepare_seek(key);
        let mem_entry = self.in_memory_cursor.seek(&key);

        if let Some((mem_key, Some(node))) = mem_entry &&
            *mem_key == key
        {
            // An exact overlay hit is the first logical entry at or after `key`, so the DB cursor
            // can stay lazy until a later operation needs it.
            if matches!(&self.db_cursor_state, DbCursorState::Positioned((db_key, _)) if db_key < &key)
            {
                self.db_cursor_state = DbCursorState::NeedsPosition;
            }

            let entry = Some((key, node.clone()));
            self.position.set_key(entry.as_ref().map(|e| e.0));
            return Ok(entry)
        }

        self.cursor_seek(key)?;

        let entry = self.choose_next_entry()?;
        self.position.set_key(entry.as_ref().map(|e| e.0));
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        debug_assert!(
            !matches!(self.position, CursorPosition::Unseeked),
            "Cursor must be seek'd before next is called",
        );

        // A missing logical result leaves the reusable request bound unchanged.
        let Some(last_key) = self.position.key() else {
            return Ok(None);
        };

        self.position.set_lower_bound(Bound::Excluded(last_key));

        // If either cursor is currently pointing to the last entry which was returned then consume
        // that entry so that `choose_next_entry` is looking at the subsequent one.
        if let Some((key, _)) = self.in_memory_cursor.current() &&
            key == &last_key
        {
            self.in_memory_cursor.first_after(&last_key);
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
        self.position.set_key(entry.as_ref().map(|e| e.0));
        Ok(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        match self.position.key() {
            Some(key) => Ok(Some(key)),
            None => self.get_cursor_mut().current(),
        }
    }

    fn reset(&mut self) {
        self.cursor.reset();
        self.in_memory_cursor.reset();

        self.db_cursor_state = DbCursorState::NeedsPosition;
        self.position = CursorPosition::Unseeked;
    }
}

impl<C: TrieStorageCursor> TrieStorageCursor for InMemoryTrieCursor<'_, C> {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.reset();
        self.cursor.set_hashed_address(hashed_address);
        let in_memory_cursor = Self::get_storage_overlay(self.trie_updates, hashed_address);
        self.in_memory_cursor = in_memory_cursor;
        self.db_cursor_state = DbCursorState::NeedsPosition;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::mock::{MockTrieCursor, MockTrieCursorFactory};
    use assert_matches::assert_matches;
    use reth_concurrency::sync::parking_lot::Mutex;
    use reth_trie_common::updates::StorageTrieUpdatesSorted;
    use std::{collections::BTreeMap, sync::Arc};
    use tracing::debug;

    #[derive(Debug)]
    struct InMemoryTrieCursorTestCase {
        db_nodes: Vec<(Nibbles, BranchNodeCompact)>,
        in_memory_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)>,
        expected_results: Vec<(Nibbles, BranchNodeCompact)>,
    }

    fn execute_test(test_case: InMemoryTrieCursorTestCase) {
        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> =
            test_case.db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);

        let trie_updates = TrieUpdatesSorted::new(test_case.in_memory_nodes, Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &trie_updates);

        let mut results = Vec::new();

        if let Some(first_expected) = test_case.expected_results.first() &&
            let Ok(Some(entry)) = cursor.seek(first_expected.0)
        {
            results.push(entry);
        }

        if !test_case.expected_results.is_empty() {
            while let Ok(Some(entry)) = cursor.next() {
                results.push(entry);
            }
        }

        assert_eq!(
            results, test_case.expected_results,
            "Results mismatch.\nGot: {:?}\nExpected: {:?}",
            results, test_case.expected_results
        );
    }

    #[reth_concurrency::test]
    fn test_empty_db_and_memory() {
        let test_case = InMemoryTrieCursorTestCase {
            db_nodes: vec![],
            in_memory_nodes: vec![],
            expected_results: vec![],
        };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_only_db_nodes() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0011, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(0b0011, 0b0010, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let test_case = InMemoryTrieCursorTestCase {
            db_nodes: db_nodes.clone(),
            in_memory_nodes: vec![],
            expected_results: db_nodes,
        };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_only_in_memory_nodes() {
        let in_memory_nodes = vec![
            (
                Nibbles::from_nibbles([0x1]),
                Some(BranchNodeCompact::new(0b0011, 0b0001, 0, vec![], None)),
            ),
            (
                Nibbles::from_nibbles([0x2]),
                Some(BranchNodeCompact::new(0b0011, 0b0010, 0, vec![], None)),
            ),
            (
                Nibbles::from_nibbles([0x3]),
                Some(BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
            ),
        ];

        let expected_results: Vec<(Nibbles, BranchNodeCompact)> = in_memory_nodes
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|node| (*k, node.clone())))
            .collect();

        let test_case =
            InMemoryTrieCursorTestCase { db_nodes: vec![], in_memory_nodes, expected_results };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_in_memory_overwrites_db() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0011, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(0b0011, 0b0010, 0, vec![], None)),
        ];

        let in_memory_nodes = vec![
            (
                Nibbles::from_nibbles([0x1]),
                Some(BranchNodeCompact::new(0b1111, 0b1111, 0, vec![], None)),
            ),
            (
                Nibbles::from_nibbles([0x3]),
                Some(BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
            ),
        ];

        let expected_results = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b1111, 0b1111, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(0b0011, 0b0010, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let test_case = InMemoryTrieCursorTestCase { db_nodes, in_memory_nodes, expected_results };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_in_memory_deletes_db_nodes() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0011, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(0b0011, 0b0010, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let in_memory_nodes = vec![(Nibbles::from_nibbles([0x2]), None)];

        let expected_results = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0011, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let test_case = InMemoryTrieCursorTestCase { db_nodes, in_memory_nodes, expected_results };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_complex_interleaving() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
            (Nibbles::from_nibbles([0x5]), BranchNodeCompact::new(0b0101, 0b0101, 0, vec![], None)),
            (Nibbles::from_nibbles([0x7]), BranchNodeCompact::new(0b0111, 0b0111, 0, vec![], None)),
        ];

        let in_memory_nodes = vec![
            (
                Nibbles::from_nibbles([0x2]),
                Some(BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)),
            ),
            (Nibbles::from_nibbles([0x3]), None),
            (
                Nibbles::from_nibbles([0x4]),
                Some(BranchNodeCompact::new(0b0100, 0b0100, 0, vec![], None)),
            ),
            (
                Nibbles::from_nibbles([0x6]),
                Some(BranchNodeCompact::new(0b0110, 0b0110, 0, vec![], None)),
            ),
            (Nibbles::from_nibbles([0x7]), None),
            (
                Nibbles::from_nibbles([0x8]),
                Some(BranchNodeCompact::new(0b1000, 0b1000, 0, vec![], None)),
            ),
        ];

        let expected_results = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)),
            (Nibbles::from_nibbles([0x4]), BranchNodeCompact::new(0b0100, 0b0100, 0, vec![], None)),
            (Nibbles::from_nibbles([0x5]), BranchNodeCompact::new(0b0101, 0b0101, 0, vec![], None)),
            (Nibbles::from_nibbles([0x6]), BranchNodeCompact::new(0b0110, 0b0110, 0, vec![], None)),
            (Nibbles::from_nibbles([0x8]), BranchNodeCompact::new(0b1000, 0b1000, 0, vec![], None)),
        ];

        let test_case = InMemoryTrieCursorTestCase { db_nodes, in_memory_nodes, expected_results };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_seek_exact() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let in_memory_nodes = vec![(
            Nibbles::from_nibbles([0x2]),
            Some(BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)),
        )];

        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys.clone());

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &trie_updates);

        let result = cursor.seek_exact(Nibbles::from_nibbles([0x2])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x2]),
                BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)
            ))
        );
        assert!(visited_keys.lock().is_empty(), "exact overlay hit should not touch the DB cursor");

        let result = cursor.seek_exact(Nibbles::from_nibbles([0x3])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x3]),
                BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)
            ))
        );

        let result = cursor.seek_exact(Nibbles::from_nibbles([0x4])).unwrap();
        assert_eq!(result, None);
    }

    #[reth_concurrency::test]
    fn test_seek_overlay_exact_hit_does_not_touch_db_until_next() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let in_memory_nodes = vec![(
            Nibbles::from_nibbles([0x2]),
            Some(BranchNodeCompact::new(0b1111, 0b1111, 0, vec![], None)),
        )];

        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys.clone());

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &trie_updates);

        let result = cursor.seek(Nibbles::from_nibbles([0x2])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x2]),
                BranchNodeCompact::new(0b1111, 0b1111, 0, vec![], None)
            ))
        );
        assert!(visited_keys.lock().is_empty(), "exact overlay hit should not touch the DB cursor");

        let result = cursor.next().unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x3]),
                BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)
            ))
        );
        assert!(!visited_keys.lock().is_empty(), "next should lazily position the DB cursor");
    }

    #[reth_concurrency::test]
    fn test_seek_overlay_exact_hit_repositions_stale_db_on_next() {
        let db_nodes = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
            (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
        ];

        let in_memory_nodes = vec![(
            Nibbles::from_nibbles([0x2]),
            Some(BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)),
        )];

        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys.clone());

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &trie_updates);

        let result = cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x1]),
                BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)
            ))
        );
        assert_eq!(visited_keys.lock().len(), 1);

        let result = cursor.seek(Nibbles::from_nibbles([0x2])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x2]),
                BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)
            ))
        );
        assert_eq!(visited_keys.lock().len(), 1, "exact overlay hit should not seek the DB");

        let result = cursor.next().unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x3]),
                BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)
            ))
        );
    }

    #[reth_concurrency::test]
    fn test_multiple_consecutive_deletes() {
        let db_nodes: Vec<(Nibbles, BranchNodeCompact)> = (1..=10)
            .map(|i| {
                (
                    Nibbles::from_nibbles([i]),
                    BranchNodeCompact::new(i as u16, i as u16, 0, vec![], None),
                )
            })
            .collect();

        let in_memory_nodes = vec![
            (Nibbles::from_nibbles([0x3]), None),
            (Nibbles::from_nibbles([0x4]), None),
            (Nibbles::from_nibbles([0x5]), None),
            (Nibbles::from_nibbles([0x6]), None),
        ];

        let expected_results = vec![
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(1, 1, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(2, 2, 0, vec![], None)),
            (Nibbles::from_nibbles([0x7]), BranchNodeCompact::new(7, 7, 0, vec![], None)),
            (Nibbles::from_nibbles([0x8]), BranchNodeCompact::new(8, 8, 0, vec![], None)),
            (Nibbles::from_nibbles([0x9]), BranchNodeCompact::new(9, 9, 0, vec![], None)),
            (Nibbles::from_nibbles([0xa]), BranchNodeCompact::new(10, 10, 0, vec![], None)),
        ];

        let test_case = InMemoryTrieCursorTestCase { db_nodes, in_memory_nodes, expected_results };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_empty_db_with_in_memory_deletes() {
        let in_memory_nodes = vec![
            (Nibbles::from_nibbles([0x1]), None),
            (
                Nibbles::from_nibbles([0x2]),
                Some(BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)),
            ),
            (Nibbles::from_nibbles([0x3]), None),
        ];

        let expected_results = vec![(
            Nibbles::from_nibbles([0x2]),
            BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None),
        )];

        let test_case =
            InMemoryTrieCursorTestCase { db_nodes: vec![], in_memory_nodes, expected_results };
        execute_test(test_case);
    }

    #[reth_concurrency::test]
    fn test_current_key_tracking() {
        let db_nodes = vec![(
            Nibbles::from_nibbles([0x2]),
            BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None),
        )];

        let in_memory_nodes = vec![
            (
                Nibbles::from_nibbles([0x1]),
                Some(BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
            ),
            (
                Nibbles::from_nibbles([0x3]),
                Some(BranchNodeCompact::new(0b0011, 0b0011, 0, vec![], None)),
            ),
        ];

        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &trie_updates);

        assert_eq!(cursor.current().unwrap(), None);

        cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x1])));

        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x2])));

        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x3])));
    }

    #[reth_concurrency::test]
    fn test_all_storage_nodes_deleted_exact_keys() {
        reth_tracing::init_test_tracing();

        // This test reproduces an edge case where:
        // - cursor is available
        // - All in-memory entries are deletions (None values)
        // - Database has corresponding entries
        // - Expected: NO leaves should be returned (all deleted)

        // Generate 42 trie node entries with keys distributed across the keyspace
        let mut db_nodes: Vec<(Nibbles, BranchNodeCompact)> = (0..10)
            .map(|i| {
                let key_bytes = vec![(i * 6) as u8, i as u8]; // Spread keys across keyspace
                let nibbles = Nibbles::unpack(key_bytes);
                (nibbles, BranchNodeCompact::new(i as u16, i as u16, 0, vec![], None))
            })
            .collect();

        db_nodes.sort_by_key(|(key, _)| *key);
        db_nodes.dedup_by_key(|(key, _)| *key);

        for (key, _) in &db_nodes {
            debug!("node at {key:?}");
        }

        // Create in-memory entries with same keys but all None values (deletions)
        let in_memory_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)> =
            db_nodes.iter().map(|(key, _)| (*key, None)).collect();

        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &trie_updates);

        // Seek to beginning should return None (all nodes are deleted)
        tracing::debug!("seeking to 0x");
        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(
            result, None,
            "Expected no entries when all nodes are deleted, but got {:?}",
            result
        );

        // Test seek operations at various positions - all should return None
        let seek_keys = vec![
            Nibbles::unpack([0x00]),
            Nibbles::unpack([0x5d]),
            Nibbles::unpack([0x5e]),
            Nibbles::unpack([0x5f]),
            Nibbles::unpack([0xc2]),
            Nibbles::unpack([0xc5]),
            Nibbles::unpack([0xc9]),
            Nibbles::unpack([0xf0]),
        ];

        for seek_key in seek_keys {
            tracing::debug!("seeking to {seek_key:?}");
            let result = cursor.seek(seek_key).unwrap();
            assert_eq!(
                result, None,
                "Expected None when seeking to {:?} but got {:?}",
                seek_key, result
            );
        }

        // next() should also always return None
        let result = cursor.next().unwrap();
        assert_eq!(result, None, "Expected None from next() but got {:?}", result);
    }

    #[reth_concurrency::test]
    fn test_seek_exact_misses_and_deletions_reuse_positions() {
        let node = BranchNodeCompact::new(1, 1, 0, vec![], None);
        let db_nodes = [1, 4, 6, 8]
            .into_iter()
            .map(|key| (Nibbles::from_nibbles([key]), node.clone()))
            .collect();
        let visited = Arc::new(Mutex::new(Vec::new()));
        let mock = MockTrieCursor::new(Arc::new(db_nodes), visited.clone());
        let updates = TrieUpdatesSorted::new(
            [4, 6].into_iter().map(|key| (Nibbles::from_nibbles([key]), None)).collect(),
            Default::default(),
        );
        let mut cursor = InMemoryTrieCursor::new_account(mock, &updates);

        assert_eq!(
            cursor.seek(Nibbles::from_nibbles([1])).unwrap(),
            Some((Nibbles::from_nibbles([1]), node.clone()))
        );
        for key in [2, 3, 4, 4] {
            assert_eq!(cursor.seek_exact(Nibbles::from_nibbles([key])).unwrap(), None);
            assert_eq!(cursor.next().unwrap(), None);
            assert_eq!(visited.lock().len(), 2);
        }
        assert_eq!(
            cursor.seek(Nibbles::from_nibbles([4])).unwrap(),
            Some((Nibbles::from_nibbles([8]), node.clone()))
        );
        assert_eq!(visited.lock().len(), 4);
        for key in [4, 5, 6, 7] {
            assert_eq!(cursor.seek_exact(Nibbles::from_nibbles([key])).unwrap(), None);
            assert_eq!(
                cursor.seek(Nibbles::from_nibbles([key])).unwrap(),
                Some((Nibbles::from_nibbles([8]), node.clone()))
            );
            assert_eq!(visited.lock().len(), 4);
        }
        assert_eq!(
            cursor.seek_exact(Nibbles::from_nibbles([8])).unwrap(),
            Some((Nibbles::from_nibbles([8]), node))
        );
        assert_eq!(visited.lock().len(), 4);
    }

    #[reth_concurrency::test]
    fn test_next_preserves_exhaustion_until_consumed_key_is_requested() {
        let node = BranchNodeCompact::new(1, 1, 0, vec![], None);
        let first = Nibbles::from_nibbles([1]);
        let db_nodes = (1..12).map(|key| (Nibbles::from_nibbles([key]), node.clone())).collect();
        let visited = Arc::new(Mutex::new(Vec::new()));
        let mock = MockTrieCursor::new(Arc::new(db_nodes), visited.clone());
        let updates = TrieUpdatesSorted::new(
            (2..12).map(|key| (Nibbles::from_nibbles([key]), None)).collect(),
            Default::default(),
        );
        let mut cursor = InMemoryTrieCursor::new_account(mock, &updates);

        assert_eq!(cursor.seek(first).unwrap(), Some((first, node.clone())));
        assert_eq!(cursor.next().unwrap(), None);
        assert_eq!(cursor.next().unwrap(), None);
        for key in 2..16 {
            assert_eq!(cursor.seek(Nibbles::from_nibbles([key])).unwrap(), None);
            assert_eq!(visited.lock().len(), 12);
        }
        assert_eq!(cursor.seek_exact(first).unwrap(), Some((first, node)));
        assert_eq!(visited.lock().len(), 13);
    }

    #[reth_concurrency::test]
    fn test_backward_seek_after_next_restores_consumed_overlay() {
        let node = BranchNodeCompact::new(1, 1, 0, vec![], None);
        let first = Nibbles::from_nibbles([1]);
        let overlay_key = Nibbles::from_nibbles([4]);
        let successor = Nibbles::from_nibbles([8]);
        let visited = Arc::new(Mutex::new(Vec::new()));
        let mock = MockTrieCursor::new(
            Arc::new([(first, node.clone()), (successor, node.clone())].into()),
            visited.clone(),
        );
        let updates =
            TrieUpdatesSorted::new(vec![(overlay_key, Some(node.clone()))], Default::default());
        let mut cursor = InMemoryTrieCursor::new_account(mock, &updates);

        assert_eq!(cursor.seek(first).unwrap(), Some((first, node.clone())));
        assert_eq!(cursor.next().unwrap(), Some((overlay_key, node.clone())));
        assert_eq!(
            cursor.seek(Nibbles::from_nibbles([2])).unwrap(),
            Some((overlay_key, node.clone()))
        );
        assert_eq!(visited.lock().len(), 2);
        assert_eq!(cursor.next().unwrap(), Some((successor, node.clone())));
        assert_eq!(cursor.seek_exact(overlay_key).unwrap(), Some((overlay_key, node.clone())));
        assert_eq!(visited.lock().len(), 2);
        assert_eq!(cursor.next().unwrap(), Some((successor, node.clone())));
        assert_eq!(visited.lock().len(), 3);
        assert_eq!(cursor.seek(first).unwrap(), Some((first, node)));
        assert_eq!(visited.lock().len(), 4);
    }

    #[reth_concurrency::test]
    fn test_address_switch_resets_seek_range_and_overlay() {
        let first_address = B256::with_last_byte(1);
        let second_address = B256::with_last_byte(2);
        let key = Nibbles::from_nibbles([1]);
        let node = BranchNodeCompact::new(1, 1, 0, vec![], None);
        let factory = MockTrieCursorFactory::new(
            Default::default(),
            [
                (first_address, [(key, node.clone())].into()),
                (second_address, [(key, node.clone())].into()),
            ]
            .into_iter()
            .collect(),
        );
        let updates = TrieUpdatesSorted::new(
            vec![],
            std::iter::once((
                first_address,
                StorageTrieUpdatesSorted { storage_nodes: vec![(key, None)] },
            ))
            .collect(),
        );
        let mock = factory.storage_trie_cursor(first_address).unwrap();
        let mut cursor = InMemoryTrieCursor::new_storage(mock, &updates, first_address);

        assert_eq!(cursor.seek(key).unwrap(), None);
        assert_eq!(factory.visited_storage_keys(first_address).len(), 2);
        cursor.set_hashed_address(second_address);
        assert_eq!(cursor.seek_exact(key).unwrap(), Some((key, node.clone())));
        assert_eq!(factory.visited_storage_keys(second_address).len(), 1);
        assert_eq!(cursor.next().unwrap(), None);
        cursor.set_hashed_address(first_address);
        assert_eq!(cursor.seek(key).unwrap(), None);
        assert_eq!(factory.visited_storage_keys(first_address).len(), 4);
        cursor.set_hashed_address(second_address);
        assert_eq!(cursor.seek(key).unwrap(), Some((key, node)));
        assert_eq!(factory.visited_storage_keys(second_address).len(), 3);
    }

    #[derive(Debug)]
    struct FailingTrieCursor {
        cursor: MockTrieCursor,
        fail_seek: bool,
        fail_next: bool,
    }

    impl TrieCursor for FailingTrieCursor {
        fn seek_exact(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            self.cursor.seek_exact(key)
        }

        fn seek(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = self.cursor.seek(key)?;
            if self.fail_seek {
                self.fail_seek = false;
                return Err(DatabaseError::Decode);
            }
            Ok(entry)
        }

        fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            let entry = self.cursor.next()?;
            if self.fail_next {
                self.fail_next = false;
                return Err(DatabaseError::Decode);
            }
            Ok(entry)
        }

        fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
            self.cursor.current()
        }

        fn reset(&mut self) {
            self.cursor.reset();
        }
    }

    #[reth_concurrency::test]
    fn test_cursor_errors_invalidate_reusable_positions() {
        for operation in 0..3 {
            let node = BranchNodeCompact::new(1, 1, 0, vec![], None);
            let first = Nibbles::from_nibbles([1]);
            let deleted = Nibbles::from_nibbles([2]);
            let successor = Nibbles::from_nibbles([3]);
            let mock = MockTrieCursor::new(
                Arc::new(
                    [(first, node.clone()), (deleted, node.clone()), (successor, node.clone())]
                        .into(),
                ),
                Default::default(),
            );
            let updates = TrieUpdatesSorted::new(vec![(deleted, None)], Default::default());
            let mut cursor = InMemoryTrieCursor::new_account(
                FailingTrieCursor {
                    cursor: mock,
                    fail_seek: operation == 0,
                    fail_next: operation != 0,
                },
                &updates,
            );

            let error = match operation {
                0 => cursor.seek_exact(first),
                1 => {
                    assert_eq!(cursor.seek(first).unwrap(), Some((first, node.clone())));
                    cursor.next()
                }
                _ => cursor.seek(deleted),
            };
            assert_matches!(error, Err(DatabaseError::Decode));
            assert_eq!(cursor.seek(first).unwrap(), Some((first, node.clone())));
            assert_eq!(cursor.next().unwrap(), Some((successor, node)));
        }
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;
        use tracing::debug;

        /// Generate a strategy for a `BranchNodeCompact` with simplified parameters.
        /// The constraints are:
        /// - `tree_mask` must be a subset of `state_mask`
        /// - `hash_mask` must be a subset of `state_mask`
        /// - `hash_mask.count_ones()` must equal `hashes.len()`
        ///
        /// To keep it simple, we use an empty hashes vec and `hash_mask` of 0.
        fn branch_node_strategy() -> impl Strategy<Value = BranchNodeCompact> {
            any::<u16>()
                .prop_flat_map(|state_mask| {
                    let tree_mask_strategy = any::<u16>().prop_map(move |tree| tree & state_mask);
                    (Just(state_mask), tree_mask_strategy)
                })
                .prop_map(|(state_mask, tree_mask)| {
                    BranchNodeCompact::new(state_mask, tree_mask, 0, vec![], None)
                })
        }

        /// Generate a sorted vector of (Nibbles, `BranchNodeCompact`) entries
        fn sorted_db_nodes_strategy() -> impl Strategy<Value = Vec<(Nibbles, BranchNodeCompact)>> {
            prop::collection::vec(
                (prop::collection::vec(0u8..16, 0..3), branch_node_strategy()),
                0..20,
            )
            .prop_map(|entries| {
                // Convert Vec<u8> to Nibbles and sort
                let mut result: Vec<(Nibbles, BranchNodeCompact)> = entries
                    .into_iter()
                    .map(|(bytes, node)| (Nibbles::from_nibbles_unchecked(bytes), node))
                    .collect();
                result.sort_by_key(|a| a.0);
                result.dedup_by(|a, b| a.0 == b.0);
                result
            })
        }

        /// Generate a sorted vector of (Nibbles, Option<BranchNodeCompact>) entries
        fn sorted_in_memory_nodes_strategy(
        ) -> impl Strategy<Value = Vec<(Nibbles, Option<BranchNodeCompact>)>> {
            prop::collection::vec(
                (prop::collection::vec(0u8..16, 0..3), prop::option::of(branch_node_strategy())),
                0..20,
            )
            .prop_map(|entries| {
                // Convert Vec<u8> to Nibbles and sort
                let mut result: Vec<(Nibbles, Option<BranchNodeCompact>)> = entries
                    .into_iter()
                    .map(|(bytes, node)| (Nibbles::from_nibbles_unchecked(bytes), node))
                    .collect();
                result.sort_by_key(|a| a.0);
                result.dedup_by(|a, b| a.0 == b.0);
                result
            })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(10000))]

            #[reth_concurrency::test]
            fn proptest_in_memory_trie_cursor(
                db_nodes in sorted_db_nodes_strategy(),
                in_memory_nodes in sorted_in_memory_nodes_strategy(),
                operations in prop::collection::vec(
                    (0u8..4, prop::collection::vec(0u8..16, 0..3)), 10..500,
                ),
            ) {
                reth_tracing::init_test_tracing();
                debug!(
                    db_paths=?db_nodes.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                    in_mem_nodes=?in_memory_nodes.iter().map(|(k, v)| (k, v.is_some())).collect::<Vec<_>>(),
                    num_operations=?operations.len(),
                    "Starting proptest!",
                );

                let mut merged = db_nodes.iter().cloned().collect::<BTreeMap<_, _>>();
                for (key, node) in &in_memory_nodes {
                    if let Some(node) = node {
                        merged.insert(*key, node.clone());
                    } else {
                        merged.remove(key);
                    }
                }

                // Include deleted and absent keys, not just surviving entries.
                let mut seek_keys = db_nodes.iter().map(|(key, _)| *key)
                    .chain(in_memory_nodes.iter().map(|(key, _)| *key))
                    .chain(operations.iter().map(|(_, key)| Nibbles::from_nibbles(key)))
                    .collect::<Vec<_>>();
                seek_keys.sort_unstable();
                seek_keys.dedup();

                let visited_keys = Arc::new(Mutex::new(Vec::new()));
                let mock = MockTrieCursor::new(
                    Arc::new(db_nodes.into_iter().collect()), visited_keys.clone(),
                );
                let updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
                let mut cursor = InMemoryTrieCursor::new_account(mock, &updates);

                // Increasing requests within a previously covered gap must not rescan the DB.
                let mut previous = None::<Option<Nibbles>>;
                for key in seek_keys {
                    let visits_before = visited_keys.lock().len();
                    let actual = cursor.seek(key).unwrap();
                    let expected = merged.range(key..).next()
                        .map(|(key, node)| (*key, node.clone()));
                    prop_assert_eq!(&actual, &expected, "monotone seek, key {:?}", key);
                    if let Some(successor) = previous &&
                        successor.is_none_or(|successor| successor >= key)
                    {
                        prop_assert_eq!(
                            visited_keys.lock().len(), visits_before,
                            "monotone seek rescanned a covered gap at {:?}", key,
                        );
                    }
                    previous = Some(expected.map(|(key, _)| key));
                }

                cursor.reset();
                let mut expected_current = None;
                let mut seeked = false;
                for (operation, key) in operations {
                    let key = Nibbles::from_nibbles(key);
                    let (actual, expected) = match operation {
                        0 => (
                            cursor.seek(key).unwrap(),
                            merged.range(key..).next().map(|(key, node)| (*key, node.clone())),
                        ),
                        1 => (
                            cursor.seek_exact(key).unwrap(),
                            merged.get(&key).map(|node| (key, node.clone())),
                        ),
                        2 if seeked => (
                            cursor.next().unwrap(),
                            expected_current.and_then(|last| {
                                merged.range((Bound::Excluded(last), Bound::Unbounded)).next()
                                    .map(|(key, node)| (*key, node.clone()))
                            }),
                        ),
                        2 => continue,
                        _ => {
                            cursor.reset();
                            expected_current = None;
                            seeked = false;
                            continue;
                        }
                    };
                    prop_assert_eq!(&actual, &expected, "operation {}, key {:?}", operation, key);
                    expected_current = expected.map(|(key, _)| key);
                    seeked = true;
                }
            }
        }
    }
}
