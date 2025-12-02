use crate::{
    prefix_set::TriePrefixSetsMut,
    updates::{TrieUpdates, TrieUpdatesSorted},
    HashedPostState, HashedPostStateSorted,
};
use alloc::sync::Arc;

/// Inputs for trie-related computations.
#[derive(Default, Debug, Clone)]
pub struct TrieInput {
    /// The collection of cached in-memory intermediate trie nodes that
    /// can be reused for computation.
    pub nodes: TrieUpdates,
    /// The in-memory overlay hashed state.
    pub state: HashedPostState,
    /// The collection of prefix sets for the computation. Since the prefix sets _always_
    /// invalidate the in-memory nodes, not all keys from `self.state` might be present here,
    /// if we have cached nodes for them.
    pub prefix_sets: TriePrefixSetsMut,
}

impl TrieInput {
    /// Create new trie input.
    pub const fn new(
        nodes: TrieUpdates,
        state: HashedPostState,
        prefix_sets: TriePrefixSetsMut,
    ) -> Self {
        Self { nodes, state, prefix_sets }
    }

    /// Create new trie input from in-memory state. The prefix sets will be constructed and
    /// set automatically.
    pub fn from_state(state: HashedPostState) -> Self {
        let prefix_sets = state.construct_prefix_sets();
        Self { nodes: TrieUpdates::default(), state, prefix_sets }
    }

    /// Create new trie input from the provided blocks, from oldest to newest. See the documentation
    /// for [`Self::extend_with_blocks`] for details.
    pub fn from_blocks<'a>(
        blocks: impl IntoIterator<Item = (&'a HashedPostState, &'a TrieUpdates)>,
    ) -> Self {
        let mut input = Self::default();
        input.extend_with_blocks(blocks);
        input
    }

    /// Create new trie input from the provided sorted blocks, from oldest to newest.
    /// Converts sorted types to unsorted for aggregation.
    pub fn from_blocks_sorted<'a>(
        blocks: impl IntoIterator<Item = (&'a HashedPostStateSorted, &'a TrieUpdatesSorted)>,
    ) -> Self {
        let mut input = Self::default();
        for (hashed_state, trie_updates) in blocks {
            // Extend directly from sorted types, avoiding intermediate HashMap allocations
            input.nodes.extend_from_sorted(trie_updates);
            input.state.extend_from_sorted(hashed_state);
        }
        input
    }

    /// Extend the trie input with the provided blocks, from oldest to newest.
    ///
    /// For blocks with missing trie updates, the trie input will be extended with prefix sets
    /// constructed from the state of this block and the state itself, **without** trie updates.
    pub fn extend_with_blocks<'a>(
        &mut self,
        blocks: impl IntoIterator<Item = (&'a HashedPostState, &'a TrieUpdates)>,
    ) {
        for (hashed_state, trie_updates) in blocks {
            self.append_cached_ref(trie_updates, hashed_state);
        }
    }

    /// Prepend another trie input to the current one.
    pub fn prepend_self(&mut self, mut other: Self) {
        core::mem::swap(&mut self.nodes, &mut other.nodes);
        self.nodes.extend(other.nodes);
        core::mem::swap(&mut self.state, &mut other.state);
        self.state.extend(other.state);
        // No need to swap prefix sets, as they will be sorted and deduplicated.
        self.prefix_sets.extend(other.prefix_sets);
    }

    /// Prepend state to the input and extend the prefix sets.
    pub fn prepend(&mut self, mut state: HashedPostState) {
        self.prefix_sets.extend(state.construct_prefix_sets());
        core::mem::swap(&mut self.state, &mut state);
        self.state.extend(state);
    }

    /// Prepend intermediate nodes and state to the input.
    /// Prefix sets for incoming state will be ignored.
    pub fn prepend_cached(&mut self, mut nodes: TrieUpdates, mut state: HashedPostState) {
        core::mem::swap(&mut self.nodes, &mut nodes);
        self.nodes.extend(nodes);
        core::mem::swap(&mut self.state, &mut state);
        self.state.extend(state);
    }

    /// Append state to the input and extend the prefix sets.
    pub fn append(&mut self, state: HashedPostState) {
        self.prefix_sets.extend(state.construct_prefix_sets());
        self.state.extend(state);
    }

    /// Append state to the input by reference and extend the prefix sets.
    pub fn append_ref(&mut self, state: &HashedPostState) {
        self.prefix_sets.extend(state.construct_prefix_sets());
        self.state.extend_ref(state);
    }

    /// Append intermediate nodes and state to the input.
    /// Prefix sets for incoming state will be ignored.
    pub fn append_cached(&mut self, nodes: TrieUpdates, state: HashedPostState) {
        self.nodes.extend(nodes);
        self.state.extend(state);
    }

    /// Append intermediate nodes and state to the input by reference.
    /// Prefix sets for incoming state will be ignored.
    pub fn append_cached_ref(&mut self, nodes: &TrieUpdates, state: &HashedPostState) {
        self.nodes.extend_ref(nodes);
        self.state.extend_ref(state);
    }

    /// This method clears the trie input nodes, state, and prefix sets.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.state.clear();
        self.prefix_sets.clear();
    }

    /// This method returns a cleared version of this trie input.
    pub fn cleared(mut self) -> Self {
        self.clear();
        self
    }
}

/// Sorted variant of [`TrieInput`] for efficient proof generation.
///
/// This type holds sorted versions of trie data structures, which eliminates the need
/// for expensive sorting operations during multiproof generation.
#[derive(Default, Debug, Clone)]
pub struct TrieInputSorted {
    /// Sorted cached in-memory intermediate trie nodes.
    pub nodes: Arc<TrieUpdatesSorted>,
    /// Sorted in-memory overlay hashed state.
    pub state: Arc<HashedPostStateSorted>,
    /// Prefix sets for computation.
    pub prefix_sets: TriePrefixSetsMut,
}

impl TrieInputSorted {
    /// Create new sorted trie input.
    pub const fn new(
        nodes: Arc<TrieUpdatesSorted>,
        state: Arc<HashedPostStateSorted>,
        prefix_sets: TriePrefixSetsMut,
    ) -> Self {
        Self { nodes, state, prefix_sets }
    }

    /// Create from unsorted [`TrieInput`] by sorting.
    pub fn from_unsorted(input: TrieInput) -> Self {
        Self {
            nodes: Arc::new(input.nodes.into_sorted()),
            state: Arc::new(input.state.into_sorted()),
            prefix_sets: input.prefix_sets,
        }
    }

    /// Prepend the given sorted trie input to this input.
    ///
    /// The trie input is prepended in the sense that it's used as a base and the current state
    /// is applied on top of it. This means that for duplicate keys, the current state (`self`)
    /// takes precedence.
    ///
    /// Sorted vectors (accounts, storage slots, trie nodes) use O(n + m) two-way merge.
    /// Storage/trie maps are combined via O(1) amortized `HashMap` operations.
    pub fn prepend_self(&mut self, base: Self) {
        // Merge nodes - self takes precedence
        if self.nodes.is_empty() && base.nodes.is_empty() {
            // Both empty, nothing to do
        } else if self.nodes.is_empty() {
            // Self is empty, just take base
            self.nodes = base.nodes;
        } else if !base.nodes.is_empty() {
            // Both have content, merge
            let mut nodes = Arc::unwrap_or_clone(core::mem::take(&mut self.nodes));
            nodes.prepend(Arc::unwrap_or_clone(base.nodes));
            self.nodes = Arc::new(nodes);
        }
        // else: base is empty, keep self as-is

        // Merge state - self takes precedence
        if self.state.is_empty() && base.state.is_empty() {
            // Both empty, nothing to do
        } else if self.state.is_empty() {
            // Self is empty, just take base
            self.state = base.state;
        } else if !base.state.is_empty() {
            // Both have content, merge
            let mut state = Arc::unwrap_or_clone(core::mem::take(&mut self.state));
            state.prepend(Arc::unwrap_or_clone(base.state));
            self.state = Arc::new(state);
        }
        // else: base is empty, keep self as-is

        // Merge prefix sets - base's prefix sets go first, then extend with self's
        if !base.prefix_sets.is_empty() {
            let mut merged_prefix_sets = base.prefix_sets;
            merged_prefix_sets.extend(core::mem::take(&mut self.prefix_sets));
            self.prefix_sets = merged_prefix_sets;
        }
        // else: base prefix sets empty, keep self as-is
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BranchNodeCompact, HashedStorageSorted, Nibbles};
    use alloy_primitives::{map::B256Map, B256, U256};
    use reth_primitives_traits::Account;

    /// Test prepend_self with disjoint accounts (no overlap)
    #[test]
    fn test_trie_input_sorted_prepend_self_disjoint_accounts() {
        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);
        let addr3 = B256::from([3; 32]);

        // Base has addr1
        let base_state = HashedPostStateSorted {
            accounts: vec![(addr1, Some(Account::default()))],
            storages: B256Map::default(),
        };
        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        // Self has addr2, addr3
        let self_state = HashedPostStateSorted {
            accounts: vec![
                (addr2, Some(Account { nonce: 1, ..Default::default() })),
                (addr3, None), // destroyed
            ],
            storages: B256Map::default(),
        };
        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(self_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Should have all 3 accounts in sorted order
        assert_eq!(input.state.accounts.len(), 3);
        assert_eq!(input.state.accounts[0].0, addr1);
        assert_eq!(input.state.accounts[1].0, addr2);
        assert_eq!(input.state.accounts[2].0, addr3);
    }

    /// Test prepend_self with overlapping accounts (self takes precedence)
    #[test]
    fn test_trie_input_sorted_prepend_self_overlapping_accounts() {
        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);

        // Base has addr1 with nonce=0, addr2 with nonce=10
        let base_state = HashedPostStateSorted {
            accounts: vec![
                (addr1, Some(Account { nonce: 0, ..Default::default() })),
                (addr2, Some(Account { nonce: 10, ..Default::default() })),
            ],
            storages: B256Map::default(),
        };
        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        // Self has addr2 with nonce=20 (should override base)
        let self_state = HashedPostStateSorted {
            accounts: vec![(addr2, Some(Account { nonce: 20, ..Default::default() }))],
            storages: B256Map::default(),
        };
        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(self_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Should have both accounts, addr2 with self's value (nonce=20)
        assert_eq!(input.state.accounts.len(), 2);
        assert_eq!(input.state.accounts[0].0, addr1);
        assert_eq!(input.state.accounts[0].1.unwrap().nonce, 0);
        assert_eq!(input.state.accounts[1].0, addr2);
        assert_eq!(input.state.accounts[1].1.unwrap().nonce, 20); // self takes precedence
    }

    /// Test prepend_self with storage merging
    #[test]
    fn test_trie_input_sorted_prepend_self_storage() {
        let addr1 = B256::from([1; 32]);
        let slot1 = B256::from([0x10; 32]);
        let slot2 = B256::from([0x20; 32]);
        let slot3 = B256::from([0x30; 32]);

        // Base has slot1, slot2
        let base_storage = HashedStorageSorted {
            storage_slots: vec![(slot1, U256::from(100)), (slot2, U256::from(200))],
            wiped: false,
        };
        let base_state = HashedPostStateSorted {
            accounts: vec![],
            storages: B256Map::from_iter([(addr1, base_storage)]),
        };
        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        // Self has slot2 (override), slot3 (new)
        let self_storage = HashedStorageSorted {
            storage_slots: vec![(slot2, U256::from(999)), (slot3, U256::from(300))],
            wiped: false,
        };
        let self_state = HashedPostStateSorted {
            accounts: vec![],
            storages: B256Map::from_iter([(addr1, self_storage)]),
        };
        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(self_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Should have all 3 slots, slot2 with self's value
        let storage = input.state.storages.get(&addr1).unwrap();
        assert_eq!(storage.storage_slots.len(), 3);
        assert_eq!(storage.storage_slots[0], (slot1, U256::from(100)));
        assert_eq!(storage.storage_slots[1], (slot2, U256::from(999))); // self takes precedence
        assert_eq!(storage.storage_slots[2], (slot3, U256::from(300)));
    }

    /// Test prepend_self with wiped storage
    #[test]
    fn test_trie_input_sorted_prepend_self_wiped_storage() {
        let addr1 = B256::from([1; 32]);
        let slot1 = B256::from([0x10; 32]);
        let slot2 = B256::from([0x20; 32]);

        // Base has slot1
        let base_storage =
            HashedStorageSorted { storage_slots: vec![(slot1, U256::from(100))], wiped: false };
        let base_state = HashedPostStateSorted {
            accounts: vec![],
            storages: B256Map::from_iter([(addr1, base_storage)]),
        };
        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        // Self has wiped=true with slot2 (base should be ignored)
        let self_storage =
            HashedStorageSorted { storage_slots: vec![(slot2, U256::from(200))], wiped: true };
        let self_state = HashedPostStateSorted {
            accounts: vec![],
            storages: B256Map::from_iter([(addr1, self_storage)]),
        };
        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(self_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Self is wiped, so base storage should be ignored
        let storage = input.state.storages.get(&addr1).unwrap();
        assert!(storage.wiped);
        assert_eq!(storage.storage_slots.len(), 1);
        assert_eq!(storage.storage_slots[0], (slot2, U256::from(200)));
    }

    /// Test prepend_self with trie nodes
    #[test]
    fn test_trie_input_sorted_prepend_self_trie_nodes() {
        let nibbles1 = Nibbles::from_nibbles_unchecked([0x01]);
        let nibbles2 = Nibbles::from_nibbles_unchecked([0x02]);
        let nibbles3 = Nibbles::from_nibbles_unchecked([0x03]);

        // Base has nibbles1, nibbles2
        let base_nodes = TrieUpdatesSorted::new(
            vec![
                (nibbles1, Some(BranchNodeCompact::default())),
                (nibbles2, Some(BranchNodeCompact::default())),
            ],
            B256Map::default(),
        );
        let base = TrieInputSorted {
            nodes: Arc::new(base_nodes),
            state: Arc::new(HashedPostStateSorted::default()),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        // Self has nibbles2 (removed), nibbles3
        let self_nodes = TrieUpdatesSorted::new(
            vec![
                (nibbles2, None), // removed
                (nibbles3, Some(BranchNodeCompact::default())),
            ],
            B256Map::default(),
        );
        let mut input = TrieInputSorted {
            nodes: Arc::new(self_nodes),
            state: Arc::new(HashedPostStateSorted::default()),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Should have all 3 nibbles, nibbles2 should be None (self takes precedence)
        let nodes = input.nodes.account_nodes_ref();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].0, nibbles1);
        assert!(nodes[0].1.is_some());
        assert_eq!(nodes[1].0, nibbles2);
        assert!(nodes[1].1.is_none()); // self takes precedence (removed)
        assert_eq!(nodes[2].0, nibbles3);
        assert!(nodes[2].1.is_some());
    }

    /// Test prepend_self with empty inputs
    #[test]
    fn test_trie_input_sorted_prepend_self_empty() {
        let addr1 = B256::from([1; 32]);

        // Test empty base
        let base = TrieInputSorted::default();
        let self_state = HashedPostStateSorted {
            accounts: vec![(addr1, Some(Account::default()))],
            storages: B256Map::default(),
        };
        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(self_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);
        assert_eq!(input.state.accounts.len(), 1);

        // Test empty self
        let base_state = HashedPostStateSorted {
            accounts: vec![(addr1, Some(Account::default()))],
            storages: B256Map::default(),
        };
        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };
        let mut input = TrieInputSorted::default();

        input.prepend_self(base);
        assert_eq!(input.state.accounts.len(), 1);
    }

    /// Test prepend_self prefix sets merging
    #[test]
    fn test_trie_input_sorted_prepend_self_prefix_sets() {
        let nibbles1 = Nibbles::from_nibbles_unchecked([0x01, 0x02]);
        let nibbles2 = Nibbles::from_nibbles_unchecked([0x03, 0x04]);

        let mut base_prefix = TriePrefixSetsMut::default();
        base_prefix.account_prefix_set.insert(nibbles1);

        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(HashedPostStateSorted::default()),
            prefix_sets: base_prefix,
        };

        let mut self_prefix = TriePrefixSetsMut::default();
        self_prefix.account_prefix_set.insert(nibbles2);

        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(HashedPostStateSorted::default()),
            prefix_sets: self_prefix,
        };

        input.prepend_self(base);

        // Both prefix sets should be merged
        let mut frozen = input.prefix_sets.account_prefix_set.freeze();
        assert!(frozen.contains(&nibbles1));
        assert!(frozen.contains(&nibbles2));
    }

    /// Test that sorted prepend produces semantically correct results:
    /// overlay (self) values must override base values for duplicate keys.
    #[test]
    fn test_sorted_prepend_correctness_accounts() {
        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);
        let addr3 = B256::from([3; 32]);

        // Base: addr1=nonce:1, addr2=nonce:2
        let base_state = HashedPostStateSorted {
            accounts: vec![
                (addr1, Some(Account { nonce: 1, ..Default::default() })),
                (addr2, Some(Account { nonce: 2, ..Default::default() })),
            ],
            storages: B256Map::default(),
        };

        // Overlay: addr2=nonce:22 (override), addr3=nonce:3 (new)
        let overlay_state = HashedPostStateSorted {
            accounts: vec![
                (addr2, Some(Account { nonce: 22, ..Default::default() })),
                (addr3, Some(Account { nonce: 3, ..Default::default() })),
            ],
            storages: B256Map::default(),
        };

        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(overlay_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Verify correctness:
        // - addr1: from base (nonce=1)
        // - addr2: from overlay (nonce=22, NOT 2)
        // - addr3: from overlay (nonce=3)
        assert_eq!(input.state.accounts.len(), 3);

        let acc1 = input.state.accounts.iter().find(|(a, _)| *a == addr1).unwrap();
        assert_eq!(acc1.1.unwrap().nonce, 1);

        let acc2 = input.state.accounts.iter().find(|(a, _)| *a == addr2).unwrap();
        assert_eq!(acc2.1.unwrap().nonce, 22); // overlay wins

        let acc3 = input.state.accounts.iter().find(|(a, _)| *a == addr3).unwrap();
        assert_eq!(acc3.1.unwrap().nonce, 3);
    }

    /// Test that sorted prepend produces correct storage results
    #[test]
    fn test_sorted_prepend_correctness_storage() {
        let addr = B256::from([1; 32]);
        let slot1 = B256::from([0x01; 32]);
        let slot2 = B256::from([0x02; 32]);
        let slot3 = B256::from([0x03; 32]);

        // Base: slot1=100, slot2=200
        let base_storage = HashedStorageSorted {
            storage_slots: vec![(slot1, U256::from(100)), (slot2, U256::from(200))],
            wiped: false,
        };
        let base_state = HashedPostStateSorted {
            accounts: vec![],
            storages: B256Map::from_iter([(addr, base_storage)]),
        };

        // Overlay: slot2=999 (override), slot3=300 (new)
        let overlay_storage = HashedStorageSorted {
            storage_slots: vec![(slot2, U256::from(999)), (slot3, U256::from(300))],
            wiped: false,
        };
        let overlay_state = HashedPostStateSorted {
            accounts: vec![],
            storages: B256Map::from_iter([(addr, overlay_storage)]),
        };

        let base = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(base_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        let mut input = TrieInputSorted {
            nodes: Arc::new(TrieUpdatesSorted::default()),
            state: Arc::new(overlay_state),
            prefix_sets: TriePrefixSetsMut::default(),
        };

        input.prepend_self(base);

        // Verify correctness:
        // - slot1: from base (100)
        // - slot2: from overlay (999, NOT 200)
        // - slot3: from overlay (300)
        let storage = input.state.storages.get(&addr).unwrap();
        assert_eq!(storage.storage_slots.len(), 3);

        let s1 = storage.storage_slots.iter().find(|(s, _)| *s == slot1).unwrap();
        assert_eq!(s1.1, U256::from(100));

        let s2 = storage.storage_slots.iter().find(|(s, _)| *s == slot2).unwrap();
        assert_eq!(s2.1, U256::from(999)); // overlay wins

        let s3 = storage.storage_slots.iter().find(|(s, _)| *s == slot3).unwrap();
        assert_eq!(s3.1, U256::from(300));
    }

    /// Test sorted path equals unsorted path for the same input data
    #[test]
    fn test_sorted_prepend_equals_unsorted_prepend() {
        use crate::HashedStorage;

        let addr1 = B256::from([1; 32]);
        let addr2 = B256::from([2; 32]);
        let slot1 = B256::from([0x10; 32]);
        let slot2 = B256::from([0x20; 32]);

        // Create base state (unsorted)
        let mut base_unsorted = HashedPostState::default();
        base_unsorted.accounts.insert(addr1, Some(Account { nonce: 1, ..Default::default() }));
        base_unsorted.accounts.insert(addr2, Some(Account { nonce: 2, ..Default::default() }));
        base_unsorted
            .storages
            .insert(addr1, HashedStorage::from_iter(false, [(slot1, U256::from(100))]));

        // Create overlay state (unsorted)
        let mut overlay_unsorted = HashedPostState::default();
        overlay_unsorted.accounts.insert(addr2, Some(Account { nonce: 22, ..Default::default() }));
        overlay_unsorted
            .storages
            .insert(addr1, HashedStorage::from_iter(false, [(slot2, U256::from(200))]));

        // Path 1: Unsorted prepend, then sort
        let mut unsorted_input = TrieInput::default();
        unsorted_input.state = overlay_unsorted.clone();
        unsorted_input.prepend_self(TrieInput {
            nodes: TrieUpdates::default(),
            state: base_unsorted.clone(),
            prefix_sets: TriePrefixSetsMut::default(),
        });
        let result_from_unsorted = unsorted_input.state.into_sorted();

        // Path 2: Sort first, then sorted prepend
        let base_sorted = TrieInputSorted::from_unsorted(TrieInput {
            nodes: TrieUpdates::default(),
            state: base_unsorted,
            prefix_sets: TriePrefixSetsMut::default(),
        });
        let mut sorted_input = TrieInputSorted::from_unsorted(TrieInput {
            nodes: TrieUpdates::default(),
            state: overlay_unsorted,
            prefix_sets: TriePrefixSetsMut::default(),
        });
        sorted_input.prepend_self(base_sorted);

        // Both paths should produce the same accounts
        assert_eq!(result_from_unsorted.accounts.len(), sorted_input.state.accounts.len());
        for (addr, acc) in &result_from_unsorted.accounts {
            let sorted_acc = sorted_input.state.accounts.iter().find(|(a, _)| a == addr);
            assert!(sorted_acc.is_some(), "Missing account {addr:?}");
            assert_eq!(sorted_acc.unwrap().1, *acc);
        }

        // Both paths should produce the same storage
        for (addr, storage) in &result_from_unsorted.storages {
            let sorted_storage = sorted_input.state.storages.get(addr);
            assert!(sorted_storage.is_some(), "Missing storage for {addr:?}");
            assert_eq!(sorted_storage.unwrap().storage_slots, storage.storage_slots);
        }
    }
}
