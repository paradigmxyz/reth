use super::{TrieCursor, TrieCursorFactory, TrieStorageCursor};
use alloy_primitives::{map::B256Map, B256};
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{
    prefix_set::{PrefixSet, TriePrefixSets},
    BranchNodeCompact, Nibbles,
};
use std::sync::Arc;

/// A [`TrieCursorFactory`] wrapper that creates cursors which invalidate cached trie hash data
/// for children whose paths match the prefix sets in a [`TriePrefixSets`].
///
/// The `destroyed_accounts` field of the prefix sets is not used by the cursor — it is only
/// relevant during trie update finalization, not during cursor traversal.
#[derive(Debug, Clone)]
pub struct MaskedTrieCursorFactory<CF> {
    /// Underlying trie cursor factory.
    cursor_factory: CF,
    /// Frozen prefix sets used for masking.
    prefix_sets: TriePrefixSets,
}

impl<CF> MaskedTrieCursorFactory<CF> {
    /// Create a new factory from an inner cursor factory and frozen prefix sets.
    pub const fn new(cursor_factory: CF, prefix_sets: TriePrefixSets) -> Self {
        Self { cursor_factory, prefix_sets }
    }
}

impl<CF: TrieCursorFactory> TrieCursorFactory for MaskedTrieCursorFactory<CF> {
    type AccountTrieCursor<'a>
        = MaskedTrieCursor<CF::AccountTrieCursor<'a>>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = MaskedTrieCursor<CF::StorageTrieCursor<'a>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.account_trie_cursor()?;
        Ok(MaskedTrieCursor::new(cursor, self.prefix_sets.account_prefix_set.clone()))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let cursor = self.cursor_factory.storage_trie_cursor(hashed_address)?;
        let prefix_set =
            self.prefix_sets.storage_prefix_sets.get(&hashed_address).cloned().unwrap_or_default();
        Ok(MaskedTrieCursor::new_storage(
            cursor,
            prefix_set,
            self.prefix_sets.storage_prefix_sets.clone(),
        ))
    }
}

/// A [`TrieCursor`] wrapper that invalidates cached trie hash data for children whose paths match
/// a [`PrefixSet`].
///
/// For each node returned by the inner cursor, hash bits are unset for children whose paths match
/// the prefix set, and the corresponding hashes are removed from the node. If a node's `hash_mask`
/// and `tree_mask` are both empty after masking, the node is skipped entirely.
#[derive(Debug)]
pub struct MaskedTrieCursor<C> {
    /// The inner cursor.
    cursor: C,
    /// Prefix set used to determine which children's hashes to invalidate.
    prefix_set: PrefixSet,
    /// Storage prefix sets for swapping on `set_hashed_address`.
    storage_prefix_sets: Option<B256Map<PrefixSet>>,
}

impl<C> MaskedTrieCursor<C> {
    /// Create a new cursor wrapping `cursor`, masking hash bits for children whose paths match
    /// `prefix_set`.
    pub const fn new(cursor: C, prefix_set: PrefixSet) -> Self {
        Self { cursor, prefix_set, storage_prefix_sets: None }
    }

    /// Create a new storage cursor that can swap its prefix set on `set_hashed_address`.
    pub const fn new_storage(
        cursor: C,
        prefix_set: PrefixSet,
        storage_prefix_sets: B256Map<PrefixSet>,
    ) -> Self {
        Self { cursor, prefix_set, storage_prefix_sets: Some(storage_prefix_sets) }
    }
}

impl<C: TrieCursor> MaskedTrieCursor<C> {
    /// Mask hash bits on a node for children whose paths match the prefix set.
    ///
    /// Returns `true` if the node should be kept, `false` if it should be skipped (both
    /// `hash_mask` and `tree_mask` are empty after masking).
    fn mask_node(&mut self, key: &Nibbles, node: &mut BranchNodeCompact) -> bool {
        if !self.prefix_set.contains(key) {
            return true;
        }

        // The subtree is modified — root hash is always invalid.
        node.root_hash = None;

        let original_hash_mask = node.hash_mask;
        if original_hash_mask.is_empty() {
            return true;
        }

        let mut new_hash_mask = original_hash_mask;
        let mut child_path = *key;
        let key_len = key.len();

        for nibble in original_hash_mask.iter() {
            child_path.truncate(key_len);
            child_path.push(nibble);

            if self.prefix_set.contains(&child_path) {
                new_hash_mask.unset_bit(nibble);
            }
        }

        if new_hash_mask != original_hash_mask {
            // Remove hashes for unset bits in-place.
            let hashes = Arc::make_mut(&mut node.hashes);
            let mut write = 0;
            for (read, nibble) in original_hash_mask.iter().enumerate() {
                if new_hash_mask.is_bit_set(nibble) {
                    hashes[write] = hashes[read];
                    write += 1;
                }
            }
            hashes.truncate(write);

            node.hash_mask = new_hash_mask;

            if node.hash_mask.is_empty() && node.tree_mask.is_empty() {
                return false;
            }
        }

        true
    }

    /// Apply masking to entries, advancing past fully-masked nodes.
    fn mask_entries(
        &mut self,
        mut entry: Option<(Nibbles, BranchNodeCompact)>,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        while let Some((key, mut node)) = entry {
            if self.mask_node(&key, &mut node) {
                return Ok(Some((key, node)));
            }
            entry = self.cursor.next()?;
        }
        Ok(None)
    }
}

impl<C: TrieCursor> TrieCursor for MaskedTrieCursor<C> {
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if let Some((key, mut node)) = self.cursor.seek_exact(key)? {
            if self.mask_node(&key, &mut node) {
                Ok(Some((key, node)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.cursor.seek(key)?;
        self.mask_entries(entry)
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let entry = self.cursor.next()?;
        self.mask_entries(entry)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        self.cursor.current()
    }

    fn reset(&mut self) {
        self.cursor.reset();
    }
}

impl<C: TrieStorageCursor> TrieStorageCursor for MaskedTrieCursor<C> {
    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.set_hashed_address(hashed_address);
        if let Some(storage_prefix_sets) = &self.storage_prefix_sets {
            self.prefix_set = storage_prefix_sets.get(&hashed_address).cloned().unwrap_or_default();
        }
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

    fn node_with_hashes(state_mask: u16, hash_mask: u16, hashes: Vec<B256>) -> BranchNodeCompact {
        BranchNodeCompact::new(state_mask, 0, hash_mask, hashes, None)
    }

    fn node_with_tree_mask(
        state_mask: u16,
        tree_mask: u16,
        hash_mask: u16,
        hashes: Vec<B256>,
    ) -> BranchNodeCompact {
        BranchNodeCompact::new(state_mask, tree_mask, hash_mask, hashes, None)
    }

    fn hash(byte: u8) -> B256 {
        B256::repeat_byte(byte)
    }

    #[test]
    fn test_seek_masks_matching_child_hashes() {
        // Node at [0x1] with children 2 and 5 hashed.
        // Prefix set marks child 2 as changed.
        let nodes = vec![(
            Nibbles::from_nibbles([0x1]),
            node_with_hashes(0b0000_0000_0010_0100, 0b0000_0000_0010_0100, vec![hash(2), hash(5)]),
        )];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x2]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        let (key, node) = result.unwrap();
        assert_eq!(key, Nibbles::from_nibbles([0x1]));
        // Hash bit 2 should be unset, only bit 5 remains.
        assert!(!node.hash_mask.is_bit_set(2));
        assert!(node.hash_mask.is_bit_set(5));
        assert_eq!(&*node.hashes, &[hash(5)]);
    }

    #[test]
    fn test_seek_skips_fully_masked_node() {
        // Node at [0x1] with only child 3 hashed, tree_mask empty.
        // Prefix set marks child 3 as changed → fully masked → skipped.
        // Node at [0x2] is unaffected → returned.
        let nodes = vec![
            (
                Nibbles::from_nibbles([0x1]),
                node_with_hashes(0b0000_0000_0000_1000, 0b0000_0000_0000_1000, vec![hash(3)]),
            ),
            (Nibbles::from_nibbles([0x2]), node(0b0000_0000_0000_0001)),
        ];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x3]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x2]), node(0b0000_0000_0000_0001))));
    }

    #[test]
    fn test_node_with_tree_mask_not_skipped() {
        // Node at [0x1] with child 3 hashed, tree_mask has bit 3 set.
        // Prefix set marks child 3 → hash cleared, but tree_mask keeps the node alive.
        let nodes = vec![(
            Nibbles::from_nibbles([0x1]),
            node_with_tree_mask(
                0b0000_0000_0000_1000,
                0b0000_0000_0000_1000,
                0b0000_0000_0000_1000,
                vec![hash(3)],
            ),
        )];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x3]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        let (key, node) = result.unwrap();
        assert_eq!(key, Nibbles::from_nibbles([0x1]));
        assert!(node.hash_mask.is_empty());
        assert!(node.tree_mask.is_bit_set(3));
        assert!(node.hashes.is_empty());
    }

    #[test]
    fn test_seek_exact_masks_hash_bits() {
        let nodes = vec![(
            Nibbles::from_nibbles([0x1]),
            node_with_tree_mask(
                0b0000_0000_0010_0100,
                0b0000_0000_0010_0100,
                0b0000_0000_0010_0100,
                vec![hash(2), hash(5)],
            ),
        )];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x5]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek_exact(Nibbles::from_nibbles([0x1])).unwrap();
        let (_, node) = result.unwrap();
        assert!(node.hash_mask.is_bit_set(2));
        assert!(!node.hash_mask.is_bit_set(5));
        assert_eq!(&*node.hashes, &[hash(2)]);
    }

    #[test]
    fn test_seek_exact_returns_none_for_fully_masked() {
        let nodes = vec![(
            Nibbles::from_nibbles([0x1]),
            node_with_hashes(0b0000_0000_0000_0100, 0b0000_0000_0000_0100, vec![hash(2)]),
        )];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x2]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek_exact(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_next_masks_and_skips() {
        // Three nodes: [0x1] unaffected, [0x2] fully masked, [0x3] unaffected.
        let nodes = vec![
            (
                Nibbles::from_nibbles([0x1]),
                node_with_hashes(0b0000_0000_0000_0010, 0b0000_0000_0000_0010, vec![hash(1)]),
            ),
            (
                Nibbles::from_nibbles([0x2]),
                node_with_hashes(0b0000_0000_0001_0000, 0b0000_0000_0001_0000, vec![hash(4)]),
            ),
            (
                Nibbles::from_nibbles([0x3]),
                node_with_hashes(0b0000_0000_0100_0000, 0b0000_0000_0100_0000, vec![hash(6)]),
            ),
        ];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x2, 0x4]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        // seek to [0x1], no match → returned unchanged.
        let result = cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        let (key, node) = result.unwrap();
        assert_eq!(key, Nibbles::from_nibbles([0x1]));
        assert_eq!(&*node.hashes, &[hash(1)]);

        // next() should skip [0x2] (fully masked), returning [0x3].
        let result = cursor.next().unwrap();
        let (key, node) = result.unwrap();
        assert_eq!(key, Nibbles::from_nibbles([0x3]));
        assert_eq!(&*node.hashes, &[hash(6)]);
    }

    #[test]
    fn test_no_match_returns_unchanged() {
        let nodes = vec![(
            Nibbles::from_nibbles([0x2]),
            node_with_hashes(0b0000_0000_0000_0010, 0b0000_0000_0000_0010, vec![hash(1)]),
        )];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x3]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        let (key, node) = result.unwrap();
        assert_eq!(key, Nibbles::from_nibbles([0x2]));
        // Unchanged — prefix set doesn't match [0x2].
        assert!(node.hash_mask.is_bit_set(1));
        assert_eq!(&*node.hashes, &[hash(1)]);
    }

    #[test]
    fn test_empty_prefix_set_returns_all_unchanged() {
        let h1 = hash(1);
        let h2 = hash(2);
        let nodes = vec![
            (
                Nibbles::from_nibbles([0x1]),
                node_with_hashes(0b0000_0000_0000_0010, 0b0000_0000_0000_0010, vec![h1]),
            ),
            (
                Nibbles::from_nibbles([0x2]),
                node_with_hashes(0b0000_0000_0000_0100, 0b0000_0000_0000_0100, vec![h2]),
            ),
        ];

        let ps = PrefixSetMut::default();
        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let r1 = cursor.seek(Nibbles::default()).unwrap().unwrap();
        assert_eq!(r1.0, Nibbles::from_nibbles([0x1]));
        assert_eq!(&*r1.1.hashes, &[h1]);

        let r2 = cursor.next().unwrap().unwrap();
        assert_eq!(r2.0, Nibbles::from_nibbles([0x2]));
        assert_eq!(&*r2.1.hashes, &[h2]);

        assert_eq!(cursor.next().unwrap(), None);
    }

    #[test]
    fn test_root_hash_cleared_on_mask() {
        let mut n =
            node_with_hashes(0b0000_0000_0010_0100, 0b0000_0000_0010_0100, vec![hash(2), hash(5)]);
        n.root_hash = Some(hash(0xFF));

        let nodes = vec![(Nibbles::from_nibbles([0x1]), n)];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x2]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let (_, node) = cursor.seek(Nibbles::default()).unwrap().unwrap();
        assert_eq!(node.root_hash, None);
    }

    #[test]
    fn test_node_without_hashes_returned_unchanged() {
        // Node with state_mask only (no hashes, no tree_mask) should pass through.
        let nodes = vec![(Nibbles::from_nibbles([0x1]), node(0b0000_0000_0000_0011))];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x0]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let result = cursor.seek(Nibbles::default()).unwrap();
        assert_eq!(result, Some((Nibbles::from_nibbles([0x1]), node(0b0000_0000_0000_0011))));
    }

    #[test]
    fn test_empty_cursor_returns_none() {
        let nodes = vec![];
        let ps = PrefixSetMut::default();
        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        assert_eq!(cursor.seek(Nibbles::default()).unwrap(), None);
    }

    #[test]
    fn test_reset_delegates() {
        let nodes =
            vec![(Nibbles::from_nibbles([0x1]), node(1)), (Nibbles::from_nibbles([0x2]), node(2))];

        let ps = PrefixSetMut::default();
        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let _ = cursor.seek(Nibbles::from_nibbles([0x1])).unwrap();
        assert_eq!(cursor.current().unwrap(), Some(Nibbles::from_nibbles([0x1])));

        cursor.reset();
        assert_eq!(cursor.current().unwrap(), None);
    }

    #[test]
    fn test_partial_mask_preserves_remaining_hashes() {
        // Node at [0x1] with children 0, 3, 7 hashed.
        // Prefix set marks children 0 and 7 as changed.
        // Only hash for child 3 should remain.
        let nodes = vec![(
            Nibbles::from_nibbles([0x1]),
            node_with_tree_mask(
                0b0000_0000_1000_1001,
                0b0000_0000_1000_1001,
                0b0000_0000_1000_1001,
                vec![hash(0), hash(3), hash(7)],
            ),
        )];

        let mut ps = PrefixSetMut::default();
        ps.insert(Nibbles::from_nibbles([0x1, 0x0]));
        ps.insert(Nibbles::from_nibbles([0x1, 0x7]));

        let inner = make_cursor(nodes);
        let mut cursor = MaskedTrieCursor::new(inner, ps.freeze());

        let (key, node) = cursor.seek(Nibbles::default()).unwrap().unwrap();
        assert_eq!(key, Nibbles::from_nibbles([0x1]));
        assert!(!node.hash_mask.is_bit_set(0));
        assert!(node.hash_mask.is_bit_set(3));
        assert!(!node.hash_mask.is_bit_set(7));
        assert_eq!(&*node.hashes, &[hash(3)]);
        assert_eq!(node.root_hash, None);
    }

    mod proptest_tests {
        use crate::{
            hashed_cursor::{mock::MockHashedCursorFactory, HashedPostStateCursorFactory},
            proof::Proof,
            trie_cursor::{
                masked::MaskedTrieCursorFactory, mock::MockTrieCursorFactory,
                noop::NoopTrieCursorFactory,
            },
            StateRoot,
        };
        use alloy_primitives::{map::B256Set, B256, U256};
        use proptest::prelude::*;
        use reth_primitives_traits::Account;
        use reth_trie_common::{HashedPostState, HashedStorage, MultiProofTargets};

        fn account_strategy() -> impl Strategy<Value = Account> {
            (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(
                |(nonce, balance, code_hash)| Account {
                    nonce,
                    balance: U256::from(balance),
                    bytecode_hash: Some(B256::from(code_hash)),
                },
            )
        }

        fn storage_value_strategy() -> impl Strategy<Value = U256> {
            any::<u64>().prop_filter("non-zero", |v| *v != 0).prop_map(U256::from)
        }

        /// Generates a base dataset of 1000 storage slots for account `B256::ZERO`,
        /// a 200-entry changeset partially overlapping with the base, and random
        /// proof targets partially overlapping with both.
        #[allow(clippy::type_complexity)]
        fn test_input_strategy(
        ) -> impl Strategy<Value = (Vec<(B256, U256)>, Account, Vec<(B256, Option<U256>)>, Vec<B256>)>
        {
            (
                // 1000 base storage slots: unique keys with non-zero values
                prop::collection::vec(
                    (any::<[u8; 32]>().prop_map(B256::from), storage_value_strategy()),
                    1000,
                ),
                account_strategy(),
                // 200 changeset entries: (key, Option<value>) where None = removal
                prop::collection::vec(
                    (
                        any::<[u8; 32]>().prop_map(B256::from),
                        prop::option::of(storage_value_strategy()),
                    ),
                    200,
                ),
                // Extra random keys for proof targets
                prop::collection::vec(any::<[u8; 32]>().prop_map(B256::from), 50),
            )
                .prop_flat_map(
                    |(base_slots, account, changeset_raw, extra_targets)| {
                        // Dedup base slots by key
                        let mut base_map = alloy_primitives::map::B256Map::default();
                        for (k, v) in &base_slots {
                            base_map.insert(*k, *v);
                        }
                        let base_deduped: Vec<(B256, U256)> =
                            base_map.iter().map(|(&k, &v)| (k, v)).collect();
                        let base_keys: Vec<B256> = base_deduped.iter().map(|(k, _)| *k).collect();

                        // Build changeset: 50% overlap with base keys, 50% new keys
                        let changeset_len = changeset_raw.len();
                        let half = changeset_len / 2;
                        let base_keys_for_overlap = base_keys.clone();

                        // Use indices to select from base keys for overlap portion
                        let overlap_indices =
                            prop::collection::vec(0..base_keys_for_overlap.len().max(1), half);

                        overlap_indices.prop_map(move |indices| {
                            let mut changeset: Vec<(B256, Option<U256>)> = Vec::new();

                            // First half: overlapping with base keys
                            for (i, (_, value)) in
                                indices.iter().zip(changeset_raw.iter()).take(half)
                            {
                                let key = if base_keys_for_overlap.is_empty() {
                                    changeset_raw[*i].0
                                } else {
                                    base_keys_for_overlap[*i % base_keys_for_overlap.len()]
                                };
                                changeset.push((key, *value));
                            }

                            // Second half: new keys from changeset_raw
                            for (key, value) in changeset_raw.iter().skip(half) {
                                changeset.push((*key, *value));
                            }

                            // Build proof targets: mix of base keys, changeset keys, and randoms
                            let changeset_keys: Vec<B256> =
                                changeset.iter().map(|(k, _)| *k).collect();
                            let mut proof_slot_targets: Vec<B256> = Vec::new();

                            // ~40% from base
                            for k in base_keys.iter().take(40) {
                                proof_slot_targets.push(*k);
                            }
                            // ~30% from changeset
                            for k in changeset_keys.iter().take(30) {
                                proof_slot_targets.push(*k);
                            }
                            // ~30% random
                            for k in extra_targets.iter().take(30) {
                                proof_slot_targets.push(*k);
                            }

                            (base_deduped.clone(), account, changeset, proof_slot_targets)
                        })
                    },
                )
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(50))]
            #[test]
            fn proptest_masked_cursor_multiproof_equivalence(
                (base_slots, account, changeset, proof_slot_targets) in test_input_strategy()
            ) {
                reth_tracing::init_test_tracing();

                let hashed_address = B256::ZERO;

                // Step 1: Create the base hashed post state with a single account
                // and 1000 storage slots.
                let base_state = HashedPostState {
                    accounts: std::iter::once((hashed_address, Some(account))).collect(),
                    storages: std::iter::once((
                        hashed_address,
                        HashedStorage::from_iter(false, base_slots),
                    ))
                    .collect(),
                };

                // Step 2: Compute trie updates from state root over the full base state.
                let base_hashed_cursor_factory =
                    MockHashedCursorFactory::from_hashed_post_state(base_state);
                let (_, trie_updates) = StateRoot::new(
                    NoopTrieCursorFactory,
                    base_hashed_cursor_factory.clone(),
                )
                .root_with_updates()
                .expect("state root computation should succeed");

                // Step 3: Create a MockTrieCursorFactory from those trie updates.
                let mock_trie_cursor_factory =
                    MockTrieCursorFactory::from_trie_updates(trie_updates);

                // Step 4: Build the changeset post state. Removals use U256::ZERO.
                let changeset_storage: Vec<(B256, U256)> = changeset
                    .iter()
                    .map(|(k, v)| (*k, v.unwrap_or(U256::ZERO)))
                    .collect();
                let changeset_state = HashedPostState {
                    accounts: std::iter::once((hashed_address, Some(account))).collect(),
                    storages: std::iter::once((
                        hashed_address,
                        HashedStorage::from_iter(false, changeset_storage),
                    ))
                    .collect(),
                };

                // Step 5: Generate prefix sets from the changeset.
                let prefix_sets_mut = changeset_state.construct_prefix_sets();

                // Step 6: Build proof targets.
                let slot_targets: B256Set = proof_slot_targets.into_iter().collect();
                let targets =
                    MultiProofTargets::from_iter([(hashed_address, slot_targets)]);

                // Step 7: Create the HashedPostStateCursorFactory overlaying changeset
                // on the base.
                let changeset_sorted = changeset_state.into_sorted();
                let overlay_cursor_factory = HashedPostStateCursorFactory::new(
                    base_hashed_cursor_factory,
                    &changeset_sorted,
                );

                // Step 8a: Approach A — prefix sets passed to Proof directly.
                let proof_a = Proof::new(
                    mock_trie_cursor_factory.clone(),
                    overlay_cursor_factory.clone(),
                )
                .with_prefix_sets_mut(prefix_sets_mut.clone());
                let multiproof_a = proof_a
                    .multiproof(targets.clone())
                    .expect("multiproof A should succeed");

                // Step 8b: Approach B — MaskedTrieCursorFactory, no prefix sets on Proof.
                let masked_trie_cursor_factory = MaskedTrieCursorFactory::new(
                    mock_trie_cursor_factory,
                    prefix_sets_mut.freeze(),
                );
                let proof_b = Proof::new(
                    masked_trie_cursor_factory,
                    overlay_cursor_factory,
                );
                let multiproof_b = proof_b
                    .multiproof(targets)
                    .expect("multiproof B should succeed");

                // Step 9: Compare results.
                assert_eq!(
                    multiproof_a, multiproof_b,
                    "multiproof with prefix sets should equal multiproof with masked cursor"
                );
            }
        }
    }
}
