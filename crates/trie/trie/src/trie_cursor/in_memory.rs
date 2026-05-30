use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use crate::{
    overlay_cursor::{DbCursorState, OverlayLayer, PositionedOverlayCursor},
    updates::TrieUpdatesSorted,
};
use alloy_primitives::{map::B256Map, B256};
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles};
use std::{marker::PhantomData, sync::Arc};

/// The trie cursor factory for the trie updates.
#[derive(Debug, Clone)]
pub struct InMemoryTrieCursorFactory<'overlay, CF, T> {
    /// Underlying trie cursor factory.
    cursor_factory: CF,
    /// Reference to sorted trie updates.
    trie_updates: T,
    _marker: PhantomData<&'overlay TrieUpdatesSorted>,
}

impl<'overlay, CF, T> InMemoryTrieCursorFactory<'overlay, CF, T> {
    /// Create a new trie cursor factory.
    pub const fn new(cursor_factory: CF, trie_updates: T) -> Self {
        Self { cursor_factory, trie_updates, _marker: PhantomData }
    }
}

impl<'overlay, CF, T> TrieCursorFactory for InMemoryTrieCursorFactory<'overlay, CF, T>
where
    CF: TrieCursorFactory + 'overlay,
    T: AsRef<TrieUpdatesOverlay>,
{
    type AccountTrieCursor<'cursor>
        = InMemoryTrieCursor<'cursor, CF::AccountTrieCursor<'cursor>>
    where
        Self: 'cursor;

    type StorageTrieCursor<'cursor>
        = InMemoryTrieCursor<'cursor, CF::StorageTrieCursor<'cursor>>
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
        let cursor = self.cursor_factory.storage_trie_cursor(hashed_address)?;
        Ok(InMemoryTrieCursor::new_storage(cursor, self.trie_updates.as_ref(), hashed_address))
    }
}

/// A cursor to iterate over trie updates and corresponding database entries.
/// It will always give precedence to earlier trie update overlays.
#[derive(Debug)]
pub struct InMemoryTrieCursor<'a, C> {
    /// The underlying cursor.
    cursor: C,
    /// The current DB cursor state.
    db_cursor_state: DbCursorState<Nibbles, BranchNodeCompact>,
    /// In-memory cursors over trie update overlays.
    in_memory_cursor: OverlayCursor<'a>,
    /// Lower-priority overlays that still need positioning after a lazy exact overlay hit.
    deferred_overlay_seek_start: Option<usize>,
    /// The key most recently returned from the Cursor.
    last_key: Option<Nibbles>,
    #[cfg(debug_assertions)]
    /// Whether an initial seek was called.
    seeked: bool,
    /// Source of trie update overlays.
    trie_updates: &'a TrieUpdatesOverlay,
}

impl<'a, C: TrieCursor> InMemoryTrieCursor<'a, C> {
    /// Create new account trie cursor from an indexed trie updates overlay.
    pub fn new_account(cursor: C, trie_updates: &'a TrieUpdatesOverlay) -> Self {
        let in_memory_cursor = trie_updates.account_overlay();
        Self {
            cursor,
            db_cursor_state: DbCursorState::new(false),
            in_memory_cursor,
            deferred_overlay_seek_start: None,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
            trie_updates,
        }
    }

    /// Create new storage trie cursor from an indexed trie updates overlay.
    pub fn new_storage(
        cursor: C,
        trie_updates: &'a TrieUpdatesOverlay,
        hashed_address: B256,
    ) -> Self {
        let (in_memory_cursor, db_wiped) = trie_updates.storage_overlay(hashed_address);
        Self {
            cursor,
            db_cursor_state: DbCursorState::new(db_wiped),
            in_memory_cursor,
            deferred_overlay_seek_start: None,
            last_key: None,
            #[cfg(debug_assertions)]
            seeked: false,
            trie_updates,
        }
    }

    /// Returns a mutable reference to the underlying cursor if it's not wiped, None otherwise.
    fn get_cursor_mut(&mut self) -> Option<&mut C> {
        (!self.db_cursor_state.is_wiped()).then_some(&mut self.cursor)
    }

    fn set_last_key(&mut self, next_entry: &Option<(Nibbles, BranchNodeCompact)>) {
        self.last_key = next_entry.as_ref().map(|e| e.0);
    }

    /// Positions the DB cursor state using the underlying cursor.
    fn cursor_seek(&mut self, key: Nibbles) -> Result<(), DatabaseError> {
        if self.db_cursor_state.is_positioned_at(&key) {
            return Ok(())
        }

        let entry = self.get_cursor_mut().map(|c| c.seek(key)).transpose()?.flatten();
        self.db_cursor_state.set_entry(entry);
        Ok(())
    }

    /// Positions the DB cursor at the first entry after `key`.
    fn cursor_first_after(&mut self, key: Nibbles) -> Result<(), DatabaseError> {
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
    fn choose_next_entry(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        loop {
            let mem_key = self.in_memory_cursor.min_current_key();
            let db_key = self.db_cursor_state.entry().map(|(key, _)| *key);
            let Some(next_key) = mem_key.into_iter().chain(db_key).min() else {
                return Ok(None);
            };

            if let Some(mem_value) =
                self.in_memory_cursor.highest_priority_value_at(&next_key).cloned()
            {
                if let Some(node) = mem_value {
                    return Ok(Some((next_key, node)))
                }

                self.in_memory_cursor.advance_key(&next_key);
                if self.db_cursor_state.entry().is_some_and(|(db_key, _)| db_key == &next_key) {
                    self.cursor_next()?;
                }
                continue;
            }

            if self.db_cursor_state.entry().is_some_and(|(db_key, _)| db_key == &next_key) {
                return Ok(self.db_cursor_state.entry().cloned())
            }
        }
    }
}

impl<C: TrieCursor> TrieCursor for InMemoryTrieCursor<'_, C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        #[cfg(debug_assertions)]
        {
            self.seeked = true;
        }

        self.deferred_overlay_seek_start = None;
        let entry = if let Some((idx, mem_value)) = self.in_memory_cursor.seek_until_exact(&key) {
            if mem_value.is_some() {
                self.deferred_overlay_seek_start = Some(idx + 1);
            }
            mem_value.clone().map(|node| (key, node))
        } else {
            let db_entry = self.get_cursor_mut().map(|c| c.seek_exact(key)).transpose()?.flatten();
            self.db_cursor_state.set_entry(db_entry);
            self.db_cursor_state.entry().cloned()
        };

        self.set_last_key(&entry);
        Ok(entry)
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        #[cfg(debug_assertions)]
        {
            self.seeked = true;
        }

        self.deferred_overlay_seek_start = None;
        match self.in_memory_cursor.seek_until_exact(&key) {
            Some((idx, Some(node))) => {
                self.deferred_overlay_seek_start = Some(idx + 1);
                let entry = Some((key, node.clone()));
                self.set_last_key(&entry);
                return Ok(entry);
            }
            Some((idx, None)) => {
                self.in_memory_cursor.seek_from(idx + 1, &key);
            }
            None => {}
        }

        self.cursor_seek(key)?;
        let entry = self.choose_next_entry()?;
        self.set_last_key(&entry);
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(self.seeked, "Cursor must be seek'd before next is called");
        }

        // A `last_key` of `None` indicates that the cursor is exhausted.
        let Some(last_key) = self.last_key else {
            return Ok(None);
        };

        if let Some(start) = self.deferred_overlay_seek_start.take() {
            self.in_memory_cursor.seek_from(start, &last_key);
        }
        self.in_memory_cursor.first_after(&last_key);

        match self.db_cursor_state.entry().map(|(db_key, _)| *db_key) {
            Some(db_key) if db_key == last_key => self.cursor_next()?,
            Some(db_key) if db_key > last_key => {}
            _ => self.cursor_first_after(last_key)?,
        }

        let entry = self.choose_next_entry()?;
        self.set_last_key(&entry);
        Ok(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        match &self.last_key {
            Some(key) => Ok(Some(*key)),
            None => Ok(self.get_cursor_mut().map(|c| c.current()).transpose()?.flatten()),
        }
    }

    fn reset(&mut self) {
        self.cursor.reset();

        self.db_cursor_state.set_entry(None);
        self.in_memory_cursor.reset();
        self.deferred_overlay_seek_start = None;
        self.last_key = None;
        #[cfg(debug_assertions)]
        {
            self.seeked = false;
        }
    }
}

impl<C: TrieStorageCursor> TrieStorageCursor for InMemoryTrieCursor<'_, C> {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.reset();
        self.cursor.set_hashed_address(hashed_address);
        let (layers, db_wiped) = self.trie_updates.storage_overlay_layers(hashed_address);
        self.in_memory_cursor.retarget(layers);
        self.db_cursor_state = DbCursorState::new(db_wiped);
    }
}

/// Trie updates overlays ordered from highest to lowest precedence.
#[derive(Clone, Debug, Default)]
pub struct TrieUpdatesOverlay {
    account_overlay: Arc<Vec<TrieOverlayLayer>>,
    storage_overlays: Arc<B256Map<TrieStorageOverlay>>,
    layer_capacity: usize,
}

impl TrieUpdatesOverlay {
    /// Create a new indexed trie updates overlay stack.
    pub fn new(updates: Vec<Arc<TrieUpdatesSorted>>) -> Self {
        let layer_capacity = updates.len();
        let account_overlay = Self::build_account_overlay(&updates);
        let storage_overlays = Self::build_storage_overlays(&updates);
        Self { account_overlay, storage_overlays, layer_capacity }
    }

    /// Returns `true` if the overlay does not contain any trie updates.
    pub fn is_empty(&self) -> bool {
        self.account_overlay.is_empty() && self.storage_overlays.is_empty()
    }

    fn build_account_overlay(updates: &[Arc<TrieUpdatesSorted>]) -> Arc<Vec<TrieOverlayLayer>> {
        Arc::new(
            updates
                .iter()
                .filter(|update| !update.account_nodes_ref().is_empty())
                .map(|update| TrieOverlayLayer::new(Arc::clone(update), update.account_nodes_ref()))
                .collect(),
        )
    }

    fn build_storage_overlays(
        updates: &[Arc<TrieUpdatesSorted>],
    ) -> Arc<B256Map<TrieStorageOverlay>> {
        let mut overlays: B256Map<TrieStorageOverlay> = B256Map::default();

        for update in updates {
            Self::push_storage_layer(&mut overlays, update);
        }

        Arc::new(overlays)
    }

    /// Add a trie updates layer at the end of the precedence stack.
    pub fn push_layer(&mut self, update: Arc<TrieUpdatesSorted>) {
        self.layer_capacity += 1;
        if !update.account_nodes_ref().is_empty() {
            Arc::make_mut(&mut self.account_overlay)
                .push(TrieOverlayLayer::new(Arc::clone(&update), update.account_nodes_ref()));
        }
        Self::push_storage_layer(Arc::make_mut(&mut self.storage_overlays), &update);
    }

    fn push_storage_layer(
        overlays: &mut B256Map<TrieStorageOverlay>,
        update: &Arc<TrieUpdatesSorted>,
    ) {
        for (hashed_address, storage) in update.storage_tries_ref() {
            let overlay = overlays.entry(*hashed_address).or_default();
            if overlay.db_wiped {
                continue;
            }

            if !storage.storage_nodes_ref().is_empty() {
                overlay
                    .layers
                    .push(TrieOverlayLayer::new(Arc::clone(update), storage.storage_nodes_ref()));
            }

            if storage.is_deleted() {
                overlay.db_wiped = true;
            }
        }
    }

    fn account_overlay(&self) -> OverlayCursor<'_> {
        OverlayCursor::with_capacity(self.account_overlay.as_slice(), self.layer_capacity)
    }

    fn storage_overlay(&self, hashed_address: B256) -> (OverlayCursor<'_>, bool) {
        let (layers, db_wiped) = self.storage_overlay_layers(hashed_address);
        (OverlayCursor::with_capacity(layers, self.layer_capacity), db_wiped)
    }

    fn storage_overlay_layers(&self, hashed_address: B256) -> (&[TrieOverlayLayer], bool) {
        let Some(overlay) = self.storage_overlays.get(&hashed_address) else {
            return (&[], false);
        };

        (overlay.layers.as_slice(), overlay.db_wiped)
    }
}

impl AsRef<Self> for TrieUpdatesOverlay {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[derive(Clone, Debug, Default)]
struct TrieStorageOverlay {
    layers: Vec<TrieOverlayLayer>,
    db_wiped: bool,
}

type OverlayCursor<'a> = PositionedOverlayCursor<'a, Nibbles, Option<BranchNodeCompact>>;
type TrieOverlayLayer = OverlayLayer<TrieUpdatesSorted, Nibbles, Option<BranchNodeCompact>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::mock::MockTrieCursor;
    use alloy_primitives::map::B256Map;
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

        let trie_updates = TrieUpdatesSorted::new(test_case.in_memory_nodes, Default::default());
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(trie_updates)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

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

    fn branch_node(id: u16) -> BranchNodeCompact {
        BranchNodeCompact::new(id, id, 0, vec![], None)
    }

    fn storage_trie_updates(
        hashed_address: B256,
        is_deleted: bool,
        storage_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)>,
    ) -> TrieUpdatesSorted {
        let mut storage_tries = B256Map::default();
        storage_tries.insert(
            hashed_address,
            crate::updates::StorageTrieUpdatesSorted { is_deleted, storage_nodes },
        );
        TrieUpdatesSorted::new(vec![], storage_tries)
    }

    fn mock_storage_cursor(
        hashed_address: B256,
        storage_tries: B256Map<BTreeMap<Nibbles, BranchNodeCompact>>,
    ) -> MockTrieCursor {
        let visited_storage_keys =
            storage_tries.keys().map(|key| (*key, Default::default())).collect();
        MockTrieCursor::new_storage(
            Arc::new(storage_tries),
            Arc::new(visited_storage_keys),
            hashed_address,
        )
        .unwrap()
    }

    fn storage_overlay_snapshot(
        overlay: &TrieUpdatesOverlay,
        hashed_address: B256,
    ) -> (Vec<Vec<(Nibbles, Option<BranchNodeCompact>)>>, bool) {
        let (layers, db_wiped) = overlay.storage_overlay_layers(hashed_address);
        (layers.iter().map(|layer| layer.entries().to_vec()).collect(), db_wiped)
    }

    #[test]
    fn test_incremental_storage_push_matches_rebuilt_overlay() {
        let hashed_address = B256::with_last_byte(1);
        let top = Arc::new(storage_trie_updates(
            hashed_address,
            false,
            vec![(Nibbles::from_nibbles([0x1]), Some(branch_node(1)))],
        ));
        let lower = Arc::new(storage_trie_updates(
            hashed_address,
            true,
            vec![(Nibbles::from_nibbles([0x2]), Some(branch_node(2)))],
        ));

        let mut incremental = TrieUpdatesOverlay::new(vec![Arc::clone(&top)]);
        incremental.push_layer(Arc::clone(&lower));
        let rebuilt = TrieUpdatesOverlay::new(vec![top, lower]);

        assert_eq!(
            storage_overlay_snapshot(&incremental, hashed_address),
            storage_overlay_snapshot(&rebuilt, hashed_address)
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
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys.clone());

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(trie_updates)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

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

    #[test]
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
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(trie_updates)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

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

    #[test]
    fn test_seek_overlay_exact_hit_does_not_seek_lower_overlays_or_db() {
        let db_nodes = vec![(
            Nibbles::from_nibbles([0x6]),
            BranchNodeCompact::new(0b0110, 0b0110, 0, vec![], None),
        )];
        let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> = db_nodes.into_iter().collect();
        let db_nodes_arc = Arc::new(db_nodes_map);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys.clone());

        let higher_priority = TrieUpdatesSorted::new(
            vec![
                (
                    Nibbles::from_nibbles([0x1]),
                    Some(BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
                ),
                (
                    Nibbles::from_nibbles([0x9]),
                    Some(BranchNodeCompact::new(0b1001, 0b1001, 0, vec![], None)),
                ),
            ],
            Default::default(),
        );
        let exact_hit = TrieUpdatesSorted::new(
            vec![(
                Nibbles::from_nibbles([0x5]),
                Some(BranchNodeCompact::new(0b0101, 0b0101, 0, vec![], None)),
            )],
            Default::default(),
        );
        let lower_priority = TrieUpdatesSorted::new(
            vec![
                (
                    Nibbles::from_nibbles([0x1]),
                    Some(BranchNodeCompact::new(0b0001, 0b0001, 0, vec![], None)),
                ),
                (
                    Nibbles::from_nibbles([0x7]),
                    Some(BranchNodeCompact::new(0b0111, 0b0111, 0, vec![], None)),
                ),
            ],
            Default::default(),
        );
        let overlay = TrieUpdatesOverlay::new(vec![
            Arc::new(higher_priority),
            Arc::new(exact_hit),
            Arc::new(lower_priority),
        ]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

        let result = cursor.seek(Nibbles::from_nibbles([0x5])).unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x5]),
                BranchNodeCompact::new(0b0101, 0b0101, 0, vec![], None)
            ))
        );
        assert!(visited_keys.lock().is_empty(), "exact overlay hit should not touch the DB cursor");

        let result = cursor.next().unwrap();
        assert_eq!(
            result,
            Some((
                Nibbles::from_nibbles([0x6]),
                BranchNodeCompact::new(0b0110, 0b0110, 0, vec![], None)
            ))
        );
        assert!(!visited_keys.lock().is_empty(), "next should lazily position the DB cursor");
    }

    #[test]
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
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(trie_updates)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

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

        let trie_updates = TrieUpdatesSorted::new(in_memory_nodes, Default::default());
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(trie_updates)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

        assert_eq!(cursor.current().unwrap(), None);

        cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x1])));

        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x2])));

        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x3])));
    }

    #[test]
    fn test_all_storage_slots_deleted_not_wiped_exact_keys() {
        use tracing::debug;
        reth_tracing::init_test_tracing();

        // This test reproduces an edge case where:
        // - cursor is not None (not wiped)
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
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(trie_updates)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

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
            cursor.reset();
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

    #[test]
    fn test_seek_reuses_exact_db_position() {
        let db_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), branch_node(1)),
            (Nibbles::from_nibbles([0x2]), branch_node(2)),
        ]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys.clone());

        let overlay = TrieUpdatesOverlay::default();
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

        assert_eq!(
            cursor.seek(Nibbles::from_nibbles([0x1])).unwrap(),
            Some((Nibbles::from_nibbles([0x1]), branch_node(1)))
        );
        assert_eq!(visited_keys.lock().len(), 1);

        assert_eq!(cursor.next().unwrap(), Some((Nibbles::from_nibbles([0x2]), branch_node(2))));
        assert_eq!(visited_keys.lock().len(), 2);

        assert_eq!(
            cursor.seek(Nibbles::from_nibbles([0x2])).unwrap(),
            Some((Nibbles::from_nibbles([0x2]), branch_node(2)))
        );
        assert_eq!(visited_keys.lock().len(), 2, "seek should reuse the exact DB position");
    }

    #[test]
    fn test_multiple_overlays_resolve_by_precedence() {
        let db_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(1, 1, 0, vec![], None)),
            (Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(2, 2, 0, vec![], None)),
            (Nibbles::from_nibbles([0x4]), BranchNodeCompact::new(4, 4, 0, vec![], None)),
        ]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);

        let newest = TrieUpdatesSorted::new(
            vec![
                (Nibbles::from_nibbles([0x2]), None),
                (
                    Nibbles::from_nibbles([0x3]),
                    Some(BranchNodeCompact::new(30, 30, 0, vec![], None)),
                ),
            ],
            Default::default(),
        );
        let oldest = TrieUpdatesSorted::new(
            vec![
                (
                    Nibbles::from_nibbles([0x1]),
                    Some(BranchNodeCompact::new(10, 10, 0, vec![], None)),
                ),
                (
                    Nibbles::from_nibbles([0x2]),
                    Some(BranchNodeCompact::new(20, 20, 0, vec![], None)),
                ),
                (Nibbles::from_nibbles([0x3]), Some(BranchNodeCompact::new(3, 3, 0, vec![], None))),
            ],
            Default::default(),
        );
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(newest), Arc::new(oldest)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

        let mut results = Vec::new();
        if let Some(entry) = cursor.seek(Nibbles::default()).unwrap() {
            results.push(entry);
            while let Some(entry) = cursor.next().unwrap() {
                results.push(entry);
            }
        }

        assert_eq!(
            results,
            vec![
                (Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(10, 10, 0, vec![], None)),
                (Nibbles::from_nibbles([0x3]), BranchNodeCompact::new(30, 30, 0, vec![], None)),
                (Nibbles::from_nibbles([0x4]), BranchNodeCompact::new(4, 4, 0, vec![], None)),
            ]
        );
    }

    #[test]
    fn test_indexed_account_overlay_resolves_by_precedence() {
        let db_nodes = BTreeMap::from([
            (Nibbles::from_nibbles([0x1]), branch_node(1)),
            (Nibbles::from_nibbles([0x2]), branch_node(2)),
            (Nibbles::from_nibbles([0x4]), branch_node(4)),
        ]);
        let db_nodes_arc = Arc::new(db_nodes);
        let visited_keys = Arc::new(Mutex::new(Vec::new()));
        let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);

        let newest = TrieUpdatesSorted::new(
            vec![
                (Nibbles::from_nibbles([0x2]), None),
                (Nibbles::from_nibbles([0x3]), Some(branch_node(30))),
            ],
            Default::default(),
        );
        let oldest = TrieUpdatesSorted::new(
            vec![
                (Nibbles::from_nibbles([0x1]), Some(branch_node(10))),
                (Nibbles::from_nibbles([0x2]), Some(branch_node(20))),
                (Nibbles::from_nibbles([0x3]), Some(branch_node(3))),
            ],
            Default::default(),
        );
        let overlay = TrieUpdatesOverlay::new(vec![Arc::new(newest), Arc::new(oldest)]);
        let mut cursor = InMemoryTrieCursor::new_account(mock_cursor, &overlay);

        let mut results = Vec::new();
        if let Some(entry) = cursor.seek(Nibbles::default()).unwrap() {
            results.push(entry);
            while let Some(entry) = cursor.next().unwrap() {
                results.push(entry);
            }
        }

        assert_eq!(
            results,
            vec![
                (Nibbles::from_nibbles([0x1]), branch_node(10)),
                (Nibbles::from_nibbles([0x3]), branch_node(30)),
                (Nibbles::from_nibbles([0x4]), branch_node(4)),
            ]
        );
    }

    #[test]
    fn test_storage_deletion_overlay_hides_lower_precedence_sources() {
        use crate::updates::StorageTrieUpdatesSorted;

        let hashed_address = B256::with_last_byte(1);
        let mut db_storage = B256Map::default();
        db_storage.insert(
            hashed_address,
            BTreeMap::from([(
                Nibbles::from_nibbles([0x4]),
                BranchNodeCompact::new(4, 4, 0, vec![], None),
            )]),
        );
        let mut visited_storage_keys = B256Map::default();
        visited_storage_keys.insert(hashed_address, Default::default());
        let mock_cursor = MockTrieCursor::new_storage(
            Arc::new(db_storage),
            Arc::new(visited_storage_keys),
            hashed_address,
        )
        .unwrap();

        let mut newest_storage = B256Map::default();
        newest_storage.insert(
            hashed_address,
            StorageTrieUpdatesSorted {
                is_deleted: false,
                storage_nodes: vec![(
                    Nibbles::from_nibbles([0x2]),
                    Some(BranchNodeCompact::new(2, 2, 0, vec![], None)),
                )],
            },
        );
        let newest = TrieUpdatesSorted::new(vec![], newest_storage);

        let mut deleting_storage = B256Map::default();
        deleting_storage.insert(
            hashed_address,
            StorageTrieUpdatesSorted {
                is_deleted: true,
                storage_nodes: vec![(
                    Nibbles::from_nibbles([0x1]),
                    Some(BranchNodeCompact::new(1, 1, 0, vec![], None)),
                )],
            },
        );
        let deleting = TrieUpdatesSorted::new(vec![], deleting_storage);

        let mut hidden_storage = B256Map::default();
        hidden_storage.insert(
            hashed_address,
            StorageTrieUpdatesSorted {
                is_deleted: false,
                storage_nodes: vec![(
                    Nibbles::from_nibbles([0x3]),
                    Some(BranchNodeCompact::new(3, 3, 0, vec![], None)),
                )],
            },
        );
        let hidden = TrieUpdatesSorted::new(vec![], hidden_storage);

        let overlay =
            TrieUpdatesOverlay::new(vec![Arc::new(newest), Arc::new(deleting), Arc::new(hidden)]);
        let mut cursor = InMemoryTrieCursor::new_storage(mock_cursor, &overlay, hashed_address);

        assert_eq!(
            cursor.seek(Nibbles::default()).unwrap(),
            Some((Nibbles::from_nibbles([0x1]), BranchNodeCompact::new(1, 1, 0, vec![], None)))
        );
        assert_eq!(
            cursor.next().unwrap(),
            Some((Nibbles::from_nibbles([0x2]), BranchNodeCompact::new(2, 2, 0, vec![], None)))
        );
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn test_indexed_storage_deletion_overlay_hides_lower_precedence_sources() {
        let hashed_address = B256::with_last_byte(1);
        let mut db_storage = B256Map::default();
        db_storage.insert(
            hashed_address,
            BTreeMap::from([(Nibbles::from_nibbles([0x4]), branch_node(4))]),
        );
        let mock_cursor = mock_storage_cursor(hashed_address, db_storage);

        let newest = storage_trie_updates(
            hashed_address,
            false,
            vec![(Nibbles::from_nibbles([0x2]), Some(branch_node(2)))],
        );
        let deleting = storage_trie_updates(
            hashed_address,
            true,
            vec![(Nibbles::from_nibbles([0x1]), Some(branch_node(1)))],
        );
        let hidden = storage_trie_updates(
            hashed_address,
            false,
            vec![(Nibbles::from_nibbles([0x3]), Some(branch_node(3)))],
        );
        let overlay =
            TrieUpdatesOverlay::new(vec![Arc::new(newest), Arc::new(deleting), Arc::new(hidden)]);
        let mut cursor = InMemoryTrieCursor::new_storage(mock_cursor, &overlay, hashed_address);

        assert_eq!(
            cursor.seek(Nibbles::default()).unwrap(),
            Some((Nibbles::from_nibbles([0x1]), branch_node(1)))
        );
        assert_eq!(cursor.next().unwrap(), Some((Nibbles::from_nibbles([0x2]), branch_node(2))));
        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn test_indexed_storage_overlay_switches_hashed_address() {
        let first_address = B256::with_last_byte(1);
        let second_address = B256::with_last_byte(2);
        let mut db_storage = B256Map::default();
        db_storage.insert(
            first_address,
            BTreeMap::from([(Nibbles::from_nibbles([0x4]), branch_node(4))]),
        );
        db_storage.insert(
            second_address,
            BTreeMap::from([(Nibbles::from_nibbles([0x5]), branch_node(5))]),
        );
        let mock_cursor = mock_storage_cursor(first_address, db_storage);

        let first_overlay = storage_trie_updates(
            first_address,
            false,
            vec![(Nibbles::from_nibbles([0x1]), Some(branch_node(1)))],
        );
        let second_overlay = storage_trie_updates(
            second_address,
            false,
            vec![(Nibbles::from_nibbles([0x2]), Some(branch_node(2)))],
        );
        let overlay =
            TrieUpdatesOverlay::new(vec![Arc::new(first_overlay), Arc::new(second_overlay)]);
        let mut cursor = InMemoryTrieCursor::new_storage(mock_cursor, &overlay, first_address);

        assert_eq!(
            cursor.seek(Nibbles::default()).unwrap(),
            Some((Nibbles::from_nibbles([0x1]), branch_node(1)))
        );
        assert_eq!(cursor.next().unwrap(), Some((Nibbles::from_nibbles([0x4]), branch_node(4))));

        cursor.set_hashed_address(second_address);

        assert_eq!(
            cursor.seek(Nibbles::default()).unwrap(),
            Some((Nibbles::from_nibbles([0x2]), branch_node(2)))
        );
        assert_eq!(cursor.next().unwrap(), Some((Nibbles::from_nibbles([0x5]), branch_node(5))));
        assert_eq!(cursor.next().unwrap(), None);
    }

    mod proptest_tests {
        use super::*;
        use proptest::prelude::*;

        #[derive(Clone, Debug)]
        enum CursorOp {
            Next,
            Seek(Nibbles),
            SeekExact(Nibbles),
        }

        /// Merge `db_nodes` with in-memory overlays. Overlay index 0 has highest priority.
        fn merge_with_overlays(
            db_nodes: &[(Nibbles, BranchNodeCompact)],
            overlays: &[Vec<(Nibbles, Option<BranchNodeCompact>)>],
        ) -> Vec<(Nibbles, BranchNodeCompact)> {
            let mut merged: BTreeMap<Nibbles, BranchNodeCompact> =
                db_nodes.iter().cloned().collect();

            for overlay in overlays.iter().rev() {
                for (key, node) in overlay {
                    match node {
                        Some(node) => {
                            merged.insert(*key, node.clone());
                        }
                        None => {
                            merged.remove(key);
                        }
                    }
                }
            }

            merged.into_iter().collect()
        }

        fn reference_seek(
            entries: &[(Nibbles, BranchNodeCompact)],
            position: &mut Option<usize>,
            key: Nibbles,
        ) -> Option<(Nibbles, BranchNodeCompact)> {
            let idx = entries.partition_point(|(entry_key, _)| entry_key < &key);
            if idx < entries.len() {
                *position = Some(idx);
                Some(entries[idx].clone())
            } else {
                *position = None;
                None
            }
        }

        fn reference_seek_exact(
            entries: &[(Nibbles, BranchNodeCompact)],
            position: &mut Option<usize>,
            key: Nibbles,
        ) -> Option<(Nibbles, BranchNodeCompact)> {
            match entries.binary_search_by_key(&key, |(entry_key, _)| *entry_key) {
                Ok(idx) => {
                    *position = Some(idx);
                    Some(entries[idx].clone())
                }
                Err(_) => {
                    *position = None;
                    None
                }
            }
        }

        fn reference_next(
            entries: &[(Nibbles, BranchNodeCompact)],
            position: &mut Option<usize>,
        ) -> Option<(Nibbles, BranchNodeCompact)> {
            let next_idx = position.and_then(|idx| idx.checked_add(1))?;

            if next_idx < entries.len() {
                *position = Some(next_idx);
                Some(entries[next_idx].clone())
            } else {
                *position = None;
                None
            }
        }

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

        fn nibbles_strategy() -> impl Strategy<Value = Nibbles> {
            prop::collection::vec(0u8..16, 0..4).prop_map(Nibbles::from_nibbles_unchecked)
        }

        /// Generate a sorted vector of (Nibbles, `BranchNodeCompact`) entries.
        fn sorted_db_nodes_strategy() -> impl Strategy<Value = Vec<(Nibbles, BranchNodeCompact)>> {
            prop::collection::vec((nibbles_strategy(), branch_node_strategy()), 0..20).prop_map(
                |entries| {
                    let mut result: Vec<(Nibbles, BranchNodeCompact)> =
                        entries.into_iter().collect();
                    result.sort_by_key(|a| a.0);
                    result.dedup_by(|a, b| a.0 == b.0);
                    result
                },
            )
        }

        /// Generate a sorted vector of (Nibbles, Option<BranchNodeCompact>) entries.
        fn sorted_in_memory_nodes_strategy(
        ) -> impl Strategy<Value = Vec<(Nibbles, Option<BranchNodeCompact>)>> {
            prop::collection::vec(
                (nibbles_strategy(), prop::option::of(branch_node_strategy())),
                0..20,
            )
            .prop_map(|entries| {
                let mut result: Vec<(Nibbles, Option<BranchNodeCompact>)> =
                    entries.into_iter().collect();
                result.sort_by_key(|a| a.0);
                result.dedup_by(|a, b| a.0 == b.0);
                result
            })
        }

        fn cursor_ops_strategy() -> impl Strategy<Value = Vec<CursorOp>> {
            prop::collection::vec(
                prop_oneof![
                    Just(CursorOp::Next),
                    nibbles_strategy().prop_map(CursorOp::Seek),
                    nibbles_strategy().prop_map(CursorOp::SeekExact),
                ],
                10..500,
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(10000))]

            #[test]
            fn proptest_in_memory_trie_cursor(
                db_nodes in sorted_db_nodes_strategy(),
                overlays in prop::collection::vec(sorted_in_memory_nodes_strategy(), 0..5),
                ops in cursor_ops_strategy(),
            ) {
                reth_tracing::init_test_tracing();
                use tracing::debug;

                debug!(
                    db_paths=?db_nodes.iter().map(|(k, _)| k).collect::<Vec<_>>(),
                    overlays=?overlays
                        .iter()
                        .map(|overlay| overlay.iter().map(|(k, v)| (k, v.is_some())).collect::<Vec<_>>())
                        .collect::<Vec<_>>(),
                    num_ops=?ops.len(),
                    "Starting proptest!",
                );

                let expected_combined = merge_with_overlays(&db_nodes, &overlays);
                let mut reference_position = None;

                // Create the InMemoryTrieCursor being tested
                let db_nodes_map: BTreeMap<Nibbles, BranchNodeCompact> =
                    db_nodes.iter().cloned().collect();
                let db_nodes_arc = Arc::new(db_nodes_map);
                let visited_keys = Arc::new(Mutex::new(Vec::new()));
                let mock_cursor = MockTrieCursor::new(db_nodes_arc, visited_keys);
                let trie_updates = overlays
                    .into_iter()
                    .map(|in_memory_nodes| TrieUpdatesSorted::new(in_memory_nodes, Default::default()))
                    .map(Arc::new)
                    .collect::<Vec<_>>();
                let overlay = TrieUpdatesOverlay::new(trie_updates);
                let mut test_cursor =
                    InMemoryTrieCursor::new_account(mock_cursor, &overlay);

                // Test: seek to the beginning first
                let control_first =
                    reference_seek(&expected_combined, &mut reference_position, Nibbles::default());
                let test_first = test_cursor.seek(Nibbles::default()).unwrap();
                debug!(
                    control=?control_first.as_ref().map(|(k, _)| k),
                    test=?test_first.as_ref().map(|(k, _)| k),
                    "Initial seek returned",
                );
                assert_eq!(control_first, test_first, "Initial seek mismatch");
                let mut seek_floor = control_first.as_ref().map(|(key, _)| *key);

                // Execute a sequence of random operations
                for op in ops {
                    if reference_position.is_none() {
                        break
                    }

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
                            let Some((key, _)) = control_result else { break };
                            seek_floor = Some(key);
                        }
                        CursorOp::Seek(key) => {
                            let key = seek_floor.map_or(key, |floor| key.max(floor));
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
                            let Some((key, _)) = control_result else { break };
                            seek_floor = Some(key);
                        }
                        CursorOp::SeekExact(key) => {
                            let key = seek_floor.map_or(key, |floor| key.max(floor));
                            let control_result =
                                reference_seek_exact(&expected_combined, &mut reference_position, key);
                            let test_result = test_cursor.seek_exact(key).unwrap();
                            debug!(
                                control=?control_result.as_ref().map(|(k, _)| k),
                                test=?test_result.as_ref().map(|(k, _)| k),
                                ?key,
                                "SeekExact returned",
                            );
                            assert_eq!(control_result, test_result, "SeekExact operation mismatch for key {:?}", key);
                            let Some((key, _)) = control_result else { break };
                            seek_floor = Some(key);
                        }
                    }
                }
            }
        }
    }
}
