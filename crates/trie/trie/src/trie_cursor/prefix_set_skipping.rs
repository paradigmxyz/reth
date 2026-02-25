use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{prefix_set::PrefixSet, BranchNodeCompact, Nibbles};

/// A [`TrieCursorFactory`] wrapper that creates cursors which skip trie nodes whose paths match a
/// given [`PrefixSet`].
#[derive(Debug, Clone)]
pub struct PrefixSetSkippingTrieCursorFactory<CF> {
    /// Underlying trie cursor factory.
    cursor_factory: CF,
    /// Account prefix set.
    account_prefix_set: PrefixSet,
    /// Storage prefix sets keyed by hashed address.
    storage_prefix_sets: alloy_primitives::map::B256Map<PrefixSet>,
}

impl<CF> PrefixSetSkippingTrieCursorFactory<CF> {
    /// Create a new factory from an inner cursor factory and frozen prefix sets.
    pub const fn new(
        cursor_factory: CF,
        account_prefix_set: PrefixSet,
        storage_prefix_sets: alloy_primitives::map::B256Map<PrefixSet>,
    ) -> Self {
        Self { cursor_factory, account_prefix_set, storage_prefix_sets }
    }
}

impl<CF: TrieCursorFactory> TrieCursorFactory for PrefixSetSkippingTrieCursorFactory<CF> {
    type AccountTrieCursor<'a>
        = PrefixSetSkippingTrieCursor<CF::AccountTrieCursor<'a>>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = PrefixSetSkippingTrieCursor<CF::StorageTrieCursor<'a>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.account_trie_cursor()?;
        Ok(PrefixSetSkippingTrieCursor::new(cursor, self.account_prefix_set.clone()))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.storage_trie_cursor(hashed_address)?;
        let prefix_set = self.storage_prefix_sets.get(&hashed_address).cloned().unwrap_or_default();
        Ok(PrefixSetSkippingTrieCursor::new(cursor, prefix_set))
    }
}

/// A [`TrieCursor`] wrapper that skips trie nodes whose paths match a [`PrefixSet`].
///
/// For `seek`, `seek_exact`, and `next` calls, if the returned node's path matches the prefix set,
/// the cursor keeps calling `next` on the inner cursor until a non-matching node is found or the
/// cursor is exhausted.
#[derive(Debug)]
pub struct PrefixSetSkippingTrieCursor<C> {
    /// The inner cursor.
    cursor: C,
    /// Prefix set used to determine which nodes to skip.
    prefix_set: PrefixSet,
}

impl<C> PrefixSetSkippingTrieCursor<C> {
    /// Create a new cursor wrapping `cursor`, skipping nodes whose paths match `prefix_set`.
    pub const fn new(cursor: C, prefix_set: PrefixSet) -> Self {
        Self { cursor, prefix_set }
    }
}

impl<C: TrieCursor> PrefixSetSkippingTrieCursor<C> {
    /// Advance past entries that match the prefix set, returning the first non-matching entry.
    fn skip_matching(
        &mut self,
        mut entry: Option<(Nibbles, BranchNodeCompact)>,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        while let Some((ref key, _)) = entry {
            if !self.prefix_set.contains(key) {
                break;
            }
            entry = self.cursor.next()?;
        }
        Ok(entry)
    }
}

impl<C: TrieCursor> TrieCursor for PrefixSetSkippingTrieCursor<C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.cursor.seek_exact(key)?;
        match entry {
            Some((ref k, _)) if self.prefix_set.contains(k) => Ok(None),
            _ => Ok(entry),
        }
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.cursor.seek(key)?;
        self.skip_matching(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.cursor.next()?;
        self.skip_matching(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.cursor.current()
    }

    fn reset(&mut self) {
        self.cursor.reset();
    }
}

impl<C: TrieStorageCursor> TrieStorageCursor for PrefixSetSkippingTrieCursor<C> {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_cursor::mock::MockTrieCursor;
    use parking_lot::Mutex;
    use reth_trie_common::prefix_set::PrefixSetMut;
    use std::{collections::BTreeMap, sync::Arc};

    fn make_cursor(nodes: Vec<(Nibbles, BranchNodeCompact)>) -> MockTrieCursor {
        let map: BTreeMap<Nibbles, BranchNodeCompact> = nodes.into_iter().collect();
        MockTrieCursor::new(Arc::new(map), Arc::new(Mutex::new(Vec::new())))
    }

    fn node(state_mask: u16) -> BranchNodeCompact {
        BranchNodeCompact::new(state_mask, 0, 0, vec![], None)
    }

    #[test]
    fn test_seek_skips_matching_nodes() {
        let nodes = vec![
            (Nibbles::from_nibbles([0x1]), node(1)),
            (Nibbles::from_nibbles([0x2]), node(2)),
            (Nibbles::from_nibbles([0x3]), node(3)),
        ];

        let mut ps = PrefixSetMut::default();
        // Mark [0x1] and [0x2] as changed — these should be skipped.
        ps.insert(Nibbles::from_nibbles([0x1]));
        ps.insert(Nibbles::from_nibbles([0x2]));

        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        // Seeking from the start should skip [0x1] and [0x2], returning [0x3].
        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x3]), node(3))));
    }

    #[test]
    fn test_seek_exact_returns_none_for_matching() {
        let nodes =
            vec![(Nibbles::from_nibbles([0x1]), node(1)), (Nibbles::from_nibbles([0x2]), node(2))];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1]));

        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        // seek_exact on a matching node returns None.
        let result = cursor.seek_exact(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(result, None);

        // seek_exact on a non-matching node returns the entry.
        let result = cursor.seek_exact(Nibbles::from_nibbles([0x2])).unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x2]), node(2))));
    }

    #[test]
    fn test_next_skips_matching_nodes() {
        let nodes = vec![
            (Nibbles::from_nibbles([0x1]), node(1)),
            (Nibbles::from_nibbles([0x2]), node(2)),
            (Nibbles::from_nibbles([0x3]), node(3)),
            (Nibbles::from_nibbles([0x4]), node(4)),
        ];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x2]));
        ps.insert(Nibbles::from_nibbles([0x3]));

        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        // Seek to [0x1] (not matching).
        let result = cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x1]), node(1))));

        // next() should skip [0x2] and [0x3], returning [0x4].
        let result = cursor.next().unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x4]), node(4))));
    }

    #[test]
    fn test_all_nodes_matching_returns_none() {
        let nodes =
            vec![(Nibbles::from_nibbles([0x1]), node(1)), (Nibbles::from_nibbles([0x2]), node(2))];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1]));
        ps.insert(Nibbles::from_nibbles([0x2]));

        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_empty_prefix_set_returns_all() {
        let nodes = vec![
            (Nibbles::from_nibbles([0x1]), node(1)),
            (Nibbles::from_nibbles([0x2]), node(2)),
            (Nibbles::from_nibbles([0x3]), node(3)),
        ];

        let ps = PrefixSetMut::default();
        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        let r1 = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(r1, Some((Nibbles::from_nibbles([0x1]), node(1))));

        let r2 = cursor.next().unwrap();
        assert_eq!(r2, Some((Nibbles::from_nibbles([0x2]), node(2))));

        let r3 = cursor.next().unwrap();
        assert_eq!(r3, Some((Nibbles::from_nibbles([0x3]), node(3))));

        let r4 = cursor.next().unwrap();
        assert_eq!(r4, None);
    }

    #[test]
    fn test_prefix_matching_skips_children() {
        // A prefix set entry [0x1] should match nodes [0x1], [0x1, 0x2], etc.
        let nodes = vec![
            (Nibbles::from_nibbles([0x1, 0x0]), node(1)),
            (Nibbles::from_nibbles([0x1, 0x5]), node(2)),
            (Nibbles::from_nibbles([0x2, 0x0]), node(3)),
        ];

        let mut ps = PrefixSetMut::default();
        // Insert a key that starts with [0x1] — `contains` checks if any key in the set
        // starts with the given prefix, so [0x1, 0x0] and [0x1, 0x5] will match prefix [0x1].
        ps.insert(Nibbles::from_nibbles([0x1, 0x0]));
        ps.insert(Nibbles::from_nibbles([0x1, 0x5]));

        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        // Seeking from start should skip [0x1, 0x0] and [0x1, 0x5], returning [0x2, 0x0].
        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x2, 0x0]), node(3))));
    }

    #[test]
    fn test_empty_cursor_returns_none() {
        let nodes = vec![];
        let ps = PrefixSetMut::default();
        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_reset_delegates() {
        let nodes =
            vec![(Nibbles::from_nibbles([0x1]), node(1)), (Nibbles::from_nibbles([0x2]), node(2))];

        let ps = PrefixSetMut::default();
        let inner = make_cursor(nodes);
        let mut cursor = PrefixSetSkippingTrieCursor::new(inner, ps.freeze());

        let _ = cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x1])));

        cursor.reset();
        assert_eq!(cursor.current().unwrap(), None);
    }
}
