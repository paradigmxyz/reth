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

    /// Append state to the input by reference and extend the prefix sets.
    pub fn append_ref(&mut self, state: &HashedPostState) {
        self.prefix_sets.extend(state.construct_prefix_sets());
        let sorted_state = state.clone().into_sorted();
        Arc::make_mut(&mut self.state).extend_ref(&sorted_state);
    }

    /// Clears all data, reusing allocations if possible via `Arc::make_mut`.
    pub fn clear(&mut self) {
        Arc::make_mut(&mut self.nodes).clear();
        Arc::make_mut(&mut self.state).clear();
        self.prefix_sets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hashed_state::HashedStorage, Nibbles};
    use alloy_primitives::{keccak256, Address, B256, U256};

    /// Test appending sorted state ref populates prefix sets and state correctly
    #[test]
    fn test_trie_input_sorted_append_ref_populates_prefix_sets_and_state() {
        // Build an unsorted hashed state with one account and one storage slot
        let address = Address::random();
        let hashed_addr = keccak256(address);
        let slot = B256::random();

        let mut state = HashedPostState::default();
        state.accounts.insert(hashed_addr, Some(Default::default()));
        state
            .storages
            .insert(hashed_addr, HashedStorage::from_iter(false, [(slot, U256::from(1))]));

        let mut input = TrieInputSorted::default();
        input.append_ref(&state);

        // Prefix sets should include the address and the slot
        let mut acct_ps = input.prefix_sets.account_prefix_set.clone().freeze();
        assert!(acct_ps.contains(&Nibbles::unpack(hashed_addr)));
        let mut storage_ps =
            input.prefix_sets.storage_prefix_sets.get(&hashed_addr).unwrap().clone().freeze();
        assert!(storage_ps.contains(&Nibbles::unpack(slot)));

        // Sorted state should contain exactly one account and one storage entry
        assert_eq!(input.state.accounts.accounts.len(), 1);
        assert!(input.state.storages.contains_key(&hashed_addr));

        // Appending the same state again should keep deduplicated contents
        input.append_ref(&state);
        assert_eq!(input.state.accounts.accounts.len(), 1);
        assert!(input.state.storages.contains_key(&hashed_addr));
    }
}
