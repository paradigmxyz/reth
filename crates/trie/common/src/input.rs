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
    use crate::HashedStorage;
    use alloy_primitives::{B256, U256};
    use reth_primitives_traits::Account;

    /// Test sorted prepend produces same result as unsorted prepend
    #[test]
    fn test_sorted_prepend_equals_unsorted_prepend() {
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
        let mut unsorted_input =
            TrieInput { state: overlay_unsorted.clone(), ..Default::default() };
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
