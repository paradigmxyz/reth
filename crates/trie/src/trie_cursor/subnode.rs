use reth_primitives::{
    trie::{nodes::CHILD_INDEX_RANGE, BranchNodeCompact, Nibbles, StoredSubNode, TrieMask},
    B256,
};

/// Cursor for iterating over a subtrie.
#[derive(Clone, Default)]
pub struct CursorSubNode {
    /// The key of the current node.
    pub key: Nibbles,
    /// The index of the next child to visit.
    pub nibble: u8,
    /// The node itself.
    pub node: BranchNodeCompact,
}

impl std::fmt::Debug for CursorSubNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorSubNode")
            .field("key", &self.key)
            .field("nibble", &self.nibble)
            .field("state_flag", &self.state_flag())
            .field("tree_flag", &self.tree_flag())
            .field("hash_flag", &self.hash_flag())
            .field("hash", &self.hash())
            .field("node", &self.node)
            .finish()
    }
}

impl From<StoredSubNode> for CursorSubNode {
    fn from(value: StoredSubNode) -> Self {
        let nibble = value.nibble.unwrap_or(0);
        let mut node = value.node.unwrap_or_default();
        node.state_mask |= TrieMask::from_nibble(nibble);
        Self { key: Nibbles::from_hex(value.key), nibble, node }
    }
}

impl From<CursorSubNode> for StoredSubNode {
    fn from(value: CursorSubNode) -> Self {
        let nibble = Some(value.nibble);
        Self { key: value.key.hex_data.to_vec(), nibble, node: Some(value.node) }
    }
}

impl CursorSubNode {
    /// Creates a new `CursorSubNode` from a key and an optional node.
    pub fn new(key: Nibbles, node: BranchNodeCompact) -> Self {
        // Find the first nibble that is set in the state mask of the node.
        let nibble = CHILD_INDEX_RANGE.clone().find(|i| node.state_mask.is_bit_set(*i)).unwrap();
        CursorSubNode { key, node, nibble }
    }

    /// Returns the full key of the current node.
    pub fn full_key(&self) -> Nibbles {
        let mut out = self.key.clone();
        out.extend([self.nibble]);
        out
    }

    /// Returns `true` if the state flag is set for the current nibble.
    pub fn state_flag(&self) -> bool {
        self.node.state_mask.is_bit_set(self.nibble)
    }

    /// Returns `true` if the tree flag is set for the current nibble.
    pub fn tree_flag(&self) -> bool {
        self.node.tree_mask.is_bit_set(self.nibble)
    }

    /// Returns `true` if the current nibble has a root hash.
    pub fn hash_flag(&self) -> bool {
        self.node.hash_mask.is_bit_set(self.nibble)
    }

    /// Returns the root hash of the current node, if it has one.
    pub fn hash(&self) -> Option<B256> {
        if self.hash_flag() {
            Some(self.node.hash_for_nibble(self.nibble))
        } else {
            None
        }
    }
}
