use crate::{prefix_set::TriePrefixSetsMut, updates::TrieUpdates, HashedPostState};

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
        blocks: impl IntoIterator<Item = (&'a HashedPostState, Option<&'a TrieUpdates>)>,
    ) -> Self {
        let mut input = Self::default();
        input.extend_with_blocks(blocks);
        input
    }

    /// Extend the trie input with the provided blocks, from oldest to newest.
    ///
    /// For blocks with missing trie updates, the trie input will be extended with prefix sets
    /// constructed from the state of this block and the state itself, **without** trie updates.
    pub fn extend_with_blocks<'a>(
        &mut self,
        blocks: impl IntoIterator<Item = (&'a HashedPostState, Option<&'a TrieUpdates>)>,
    ) {
        for (hashed_state, trie_updates) in blocks {
            if let Some(nodes) = trie_updates.as_ref() {
                self.append_cached_ref(nodes, hashed_state);
            } else {
                self.append_ref(hashed_state);
            }
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
}
