use crate::{prefix_set::TriePrefixSetsMut, updates::TrieUpdates, HashedPostState};

/// Inputs for trie-related computations.
#[derive(Default, Debug)]
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

    /// Prepend state to the input and extend the prefix sets.
    pub fn prepend(&mut self, mut state: HashedPostState) {
        self.prefix_sets.extend(state.construct_prefix_sets());
        std::mem::swap(&mut self.state, &mut state);
        self.state.extend(state);
    }

    /// Prepend intermediate nodes and state to the input.
    /// Prefix sets for incoming state will be ignored.
    pub fn prepend_cached(&mut self, mut nodes: TrieUpdates, mut state: HashedPostState) {
        std::mem::swap(&mut self.nodes, &mut nodes);
        self.nodes.extend(nodes);
        std::mem::swap(&mut self.state, &mut state);
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
