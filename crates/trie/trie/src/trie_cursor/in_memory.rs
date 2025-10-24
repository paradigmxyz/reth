use super::{TrieCursor, TrieCursorFactory};
use crate::{forward_cursor::ForwardInMemoryCursor, updates::TrieUpdatesSorted};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles};

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
        Ok(InMemoryTrieCursor::new(Some(cursor), self.trie_updates.as_ref().account_nodes_ref()))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        // if the storage trie has no updates then we use this as the in-memory overlay.
        static EMPTY_UPDATES: Vec<(Nibbles, Option<BranchNodeCompact>)> = Vec::new();

        let storage_trie_updates = self.trie_updates.as_ref().storage_tries.get(&hashed_address);
        let (storage_nodes, cleared) = storage_trie_updates
            .map(|u| (u.storage_nodes_ref(), u.is_deleted()))
            .unwrap_or((&EMPTY_UPDATES, false));

        let cursor = if cleared {
            None
        } else {
            Some(self.cursor_factory.storage_trie_cursor(hashed_address)?)
        };

        Ok(InMemoryTrieCursor::new(cursor, storage_nodes))
    }
}

/// A cursor to iterate over trie updates and corresponding database entries.
/// It will always give precedence to the data from the trie updates.
#[derive(Debug)]
pub struct InMemoryTrieCursor<'a, C> {
    /// The underlying cursor. If None then it is assumed there is no DB data.
    cursor: Option<C>,
    /// Entry that `cursor` is currently pointing to.
    cursor_entry: Option<(Nibbles, BranchNodeCompact)>,
    /// Forward-only in-memory cursor over storage trie nodes.
    in_memory_cursor: ForwardInMemoryCursor<'a, Nibbles, Option<BranchNodeCompact>>,
    /// The key most recently returned from the Cursor.
    last_key: Option<Nibbles>,
    #[cfg(debug_assertions)]
    /// Whether an initial seek was called.
    seeked: bool,
}

impl<'a, C: TrieCursor> InMemoryTrieCursor<'a, C> {
    /// Create new trie cursor which combines a DB cursor (None to assume empty DB) and a set of
    /// in-memory trie nodes.
    pub fn new(
        cursor: Option<C>,
        trie_updates: &'a [(Nibbles, Option<BranchNodeCompact>)],
    ) -> Self {
        let in_memory_cursor = ForwardInMemoryCursor::new(trie_updates);
        Self {
            cursor,
            cursor_entry: None,
            in_memory_cursor,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
        }
    }

    /// Asserts that the next entry to be returned from the cursor is not previous to the last entry
    /// returned.
    fn set_last_key(&mut self, next_entry: &Option<(Nibbles, BranchNodeCompact)>) {
        let next_key = next_entry.as_ref().map(|e| e.0);
        assert!(
            self.last_key.is_none_or(|last| next_key.is_none_or(|next| next >= last)),
            "Cannot return entry {:?} previous to the last returned entry at {:?}",
            next_key,
            self.last_key,
        );
        self.last_key = next_key;
    }

    /// Seeks the `entry` field of the struct using the cursor.
    fn cursor_seek(&mut self, key: Nibbles, exact: bool) -> Result<(), DatabaseError> {
        if let Some(entry) = self.cursor_entry.as_ref() &&
            entry.0 >= key
        {
            // If already seeked to the given key then don't do anything. Also if we're seeked past
            // the given key then don't anything, because `TrieCursor` is specifically a
            // forward-only cursor.
        } else if exact {
            self.cursor_entry =
                self.cursor.as_mut().map(|c| c.seek_exact(key)).transpose()?.flatten();
        } else {
            self.cursor_entry = self.cursor.as_mut().map(|c| c.seek(key)).transpose()?.flatten();
        }

        Ok(())
    }

    /// Seeks the `entry` field of the struct to the subsequent entry using the cursor.
    fn cursor_next(&mut self) -> Result<(), DatabaseError> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.seeked);
        }

        // If the previous entry is `None`, and we've done a seek previously, then the cursor is
        // exhausted and we shouldn't call `next` again.
        if self.cursor_entry.is_some() {
            self.cursor_entry = self.cursor.as_mut().map(|c| c.next()).transpose()?.flatten();
        }
        Ok(())
    }

    /// Compares the current in-memory entry with the current entry of the cursor, and applies the
    /// in-memory entry to the cursor entry as an overlay. This will consume and move forward the
    /// current entries as-necessary to produce a "next" entry.
    fn next_inner(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        loop {
            match (self.in_memory_cursor.current(), &self.cursor_entry) {
                (Some((mem_key, None)), _)
                    if self.cursor_entry.as_ref().is_none_or(|(db_key, _)| mem_key < db_key) =>
                {
                    // If overlay has a removed node but DB cursor is exhausted or ahead of the
                    // in-memory cursor then move ahead in-memory, as there might be further
                    // non-removed overlay nodes.
                    self.in_memory_cursor.next();
                }
                (Some((mem_key, None)), Some((db_key, _))) if mem_key == db_key => {
                    // If overlay has a removed node which is returned from DB then move both
                    // cursors ahead to the next key.
                    self.in_memory_cursor.next();
                    self.cursor_next()?;
                }
                (Some((mem_key, Some(node))), _)
                    if self.cursor_entry.as_ref().is_none_or(|(db_key, _)| mem_key <= db_key) =>
                {
                    // If overlay returns a node prior to the DB's node, or the DB is exhausted,
                    // then we consume and return the overlay's node.
                    let this_entry = (*mem_key, node.clone());

                    // If the db entry is at the same key as the in-memory cursor then we want to
                    // consume it as well.
                    if self.cursor_entry.as_ref().is_none_or(|(db_key, _)| mem_key == db_key) {
                        self.cursor_next()?;
                    }

                    self.in_memory_cursor.next();

                    return Ok(Some(this_entry))
                }
                // All other cases:
                // - mem_key > db_key
                // - overlay is exhausted
                // Consume and return the db_entry. If DB is also exhausted then this returns None.
                _ => {
                    let this_entry = self.cursor_entry.clone();
                    if this_entry.is_some() {
                        self.cursor_next()?;
                    }
                    return Ok(this_entry)
                }
            }
        }
    }
}

impl<C: TrieCursor> TrieCursor for InMemoryTrieCursor<'_, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.cursor_seek(key, true)?;
        let mem_entry = self.in_memory_cursor.seek(&key);

        #[cfg(debug_assertions)]
        {
            self.seeked = true;
        }

        let entry = match (mem_entry, &self.cursor_entry) {
            (Some((mem_key, entry_inner)), _) if mem_key == &key => {
                entry_inner.clone().map(|node| (key, node))
            }
            (_, Some((db_key, node))) if db_key == &key => Some((key, node.clone())),
            _ => None,
        };

        self.set_last_key(&entry);
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.cursor_seek(key, false)?;
        self.in_memory_cursor.seek(&key);

        #[cfg(debug_assertions)]
        {
            self.seeked = true;
        }

        let entry = self.next_inner()?;
        self.set_last_key(&entry);
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.seeked, "Cursor must be seek'd before next is called");
        }

        // A `last_key` of `None` indicates that the cursor was never seeked, or is exhausted. If it
        // was never seeked then `next` would be undefined, just return `None`.
        if self.last_key.is_none() {
            return Ok(None);
        }

        let entry = self.next_inner()?;
        self.set_last_key(&entry);
        Ok(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        match &self.last_key {
            Some(key) => Ok(Some(*key)),
            None => Ok(self.cursor.as_mut().map(|c| c.current()).transpose()?.flatten()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::mock::MockTrieCursor;
    use parking_lot::Mutex;
    use std::{collections::BTreeMap, sync::Arc};

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

        let mut cursor = InMemoryTrieCursor::new(Some(mock_cursor), &test_case.in_memory_nodes);

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

    #[test]
    fn test_empty_db_and_memory() {
        let test_case = InMemoryTrieCursorTestCase {
            db_nodes: vec![],
            in_memory_nodes: vec![],
            expected_results: vec![],
        };
        execute_test(test_case);
    }

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);

        let mut cursor = InMemoryTrieCursor::new(Some(mock_cursor), &in_memory_nodes);

        let result = cursor.seek_exact(Nibbles::from_nibbles([0x2])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x2]),
                BranchNodeCompact::new(0b0010, 0b0010, 0, vec![], None)
            ))
        );

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

    #[test]
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

    #[test]
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

    #[test]
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

        let mut cursor = InMemoryTrieCursor::new(Some(mock_cursor), &in_memory_nodes);

        assert_eq!(cursor.current().unwrap(), None);

        cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x1])));

        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x2])));

        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x3])));
    }
}
