use alloy_primitives::map::{HashMap, HashSet};
use alloy_trie::{BranchNodeCompact, Nibbles};

/// Tracks modifications to the sparse trie structure.
///
/// Maintains references to both modified and pruned/removed branches, enabling
/// one to make batch updates to a persistent database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SparseTrieUpdates {
    pub updated_nodes: HashMap<Nibbles, BranchNodeCompact>,
    pub removed_nodes: HashSet<Nibbles>,
    pub wiped: bool,
}

impl SparseTrieUpdates {
    /// Create new wiped sparse trie updates.
    pub fn wiped() -> Self {
        Self { wiped: true, ..Default::default() }
    }

    /// Clears the updates, but keeps the backing data structures allocated.
    ///
    /// Sets `wiped` to `false`.
    pub fn clear(&mut self) {
        self.updated_nodes.clear();
        self.removed_nodes.clear();
        self.wiped = false;
    }
}
