//! Preserved sparse trie for reuse across payload validations.

use alloy_primitives::B256;
use reth_trie_sparse::SparseStateTrie;
use tracing::debug;

/// Type alias for the sparse trie type used in preservation.
pub type SparseTrie = SparseStateTrie;

/// Guard that holds the lock on the preserved trie.
/// While held, the next trie take will block. Call `store()` to save the trie before dropping.
#[derive(Debug)]
pub struct PreservedTrieGuard<'a>(parking_lot::MutexGuard<'a, PreservedSparseTrieState>);

impl<'a> PreservedTrieGuard<'a> {
    /// Creates a new guard from the preserved trie lock.
    pub(crate) const fn new(guard: parking_lot::MutexGuard<'a, PreservedSparseTrieState>) -> Self {
        PreservedTrieGuard(guard)
    }

    /// Stores a preserved trie for later reuse.
    pub fn store(&mut self, trie: PreservedSparseTrie) {
        self.0.store(trie);
    }

    /// Clears any preserved sparse trie state.
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Current state of the sparse trie owned by the overlay manager.
#[derive(Debug)]
pub(crate) enum PreservedSparseTrieState {
    /// No sparse trie has been preserved yet.
    Empty,
    /// A sparse trie is available for reuse.
    Available(PreservedSparseTrie),
    /// A sparse trie has been taken by a state-root task.
    InUse {
        /// The state root the sparse trie was anchored to when it was taken.
        state_root: B256,
    },
}

impl Default for PreservedSparseTrieState {
    fn default() -> Self {
        Self::Empty
    }
}

impl PreservedSparseTrieState {
    /// Takes the available preserved sparse trie, leaving behind its previous anchor.
    pub(crate) fn take(&mut self) -> Option<PreservedSparseTrie> {
        match core::mem::replace(self, Self::Empty) {
            Self::Available(trie) => {
                *self = Self::InUse { state_root: trie.state_root() };
                Some(trie)
            }
            state => {
                *self = state;
                None
            }
        }
    }

    /// Stores an available preserved trie.
    fn store(&mut self, trie: PreservedSparseTrie) {
        *self = Self::Available(trie);
    }

    /// Clears the sparse trie state.
    fn clear(&mut self) {
        *self = Self::Empty;
    }

    /// Returns the state root the sparse trie is or was anchored to.
    pub(crate) const fn state_root(&self) -> Option<B256> {
        match self {
            Self::Empty => None,
            Self::Available(trie) => Some(trie.state_root()),
            Self::InUse { state_root } => Some(*state_root),
        }
    }

    /// Returns true if the sparse trie is or was anchored to the given parent state root.
    pub(crate) fn is_based_on_parent_state_root(&self, parent_state_root: B256) -> bool {
        self.state_root().is_some_and(|state_root| state_root == parent_state_root)
    }
}

/// A preserved sparse trie that can be reused across payload validations.
#[derive(Debug)]
pub struct PreservedSparseTrie {
    /// The preserved sparse state trie.
    trie: SparseTrie,
    /// The state root this trie represents.
    ///
    /// Used to verify continuity: a new payload's `parent_state_root` must match this before the
    /// existing sparse trie nodes can be reused.
    state_root: B256,
}

impl PreservedSparseTrie {
    /// Creates a new anchored preserved trie.
    ///
    /// The `state_root` is the computed state root from the trie, which becomes the
    /// anchor for determining if subsequent payloads can reuse this trie.
    pub const fn anchored(trie: SparseTrie, state_root: B256) -> Self {
        Self { trie, state_root }
    }

    /// Returns the state root this trie is anchored to.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Returns true if this trie is anchored to the given parent state root.
    pub fn is_based_on_parent_state_root(&self, parent_state_root: B256) -> bool {
        self.state_root == parent_state_root
    }

    /// Consumes self and returns the trie if it can be reused for the parent state root.
    ///
    /// If the parent state root does not match the preserved trie's anchor, this drops the trie and
    /// returns `None` so the caller can create a fresh sparse trie.
    pub fn into_trie_for(self, parent_state_root: B256) -> Option<SparseTrie> {
        if self.state_root == parent_state_root {
            debug!(
                target: "engine::tree::payload_processor",
                state_root = %self.state_root,
                "Reusing anchored sparse trie for continuation payload"
            );
            Some(self.trie)
        } else {
            debug!(
                target: "engine::tree::payload_processor",
                anchor_root = %self.state_root,
                %parent_state_root,
                "Dropping anchored sparse trie - parent state root mismatch"
            );
            None
        }
    }
}
