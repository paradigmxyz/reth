use crate::{nodes::CHILD_INDEX_RANGE, Nibbles};
use reth_primitives::{trie::BranchNodeCompact, H256};

/// Cursor for iterating over a subtrie.
#[derive(Clone)]
pub struct CursorSubNode {
    /// The key of the current node.
    pub key: Nibbles,
    /// The index of the next child to visit.
    pub nibble: i8,
    /// The node itself.
    pub node: Option<BranchNodeCompact>,
}

impl Default for CursorSubNode {
    fn default() -> Self {
        Self::new(Nibbles::default(), None)
    }
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
            .finish()
    }
}

impl CursorSubNode {
    /// Creates a new `CursorSubNode` from a key and an optional node.
    pub fn new(key: Nibbles, node: Option<BranchNodeCompact>) -> Self {
        // Find the first nibble that is set in the state mask of the node.
        let nibble = match &node {
            Some(n) if n.root_hash.is_none() => {
                CHILD_INDEX_RANGE.clone().find(|i| n.state_mask.is_bit_set(*i)).unwrap() as i8
            }
            _ => -1,
        };
        CursorSubNode { key, node, nibble }
    }

    /// Returns the full key of the current node.
    pub fn full_key(&self) -> Nibbles {
        let mut out = self.key.clone();
        if self.nibble >= 0 {
            out.extend([self.nibble as u8]);
        }
        out
    }

    /// Returns `true` if the state flag is set for the current nibble.
    pub fn state_flag(&self) -> bool {
        if let Some(node) = &self.node {
            if self.nibble >= 0 {
                return node.state_mask.is_bit_set(self.nibble as u8)
            }
        }
        true
    }

    /// Returns `true` if the tree flag is set for the current nibble.
    pub fn tree_flag(&self) -> bool {
        if let Some(node) = &self.node {
            if self.nibble >= 0 {
                return node.tree_mask.is_bit_set(self.nibble as u8)
            }
        }
        true
    }

    /// Returns `true` if the current nibble has a root hash.
    pub fn hash_flag(&self) -> bool {
        match &self.node {
            Some(node) => match self.nibble {
                // This guy has it
                -1 => node.root_hash.is_some(),
                // Or get it from the children
                _ => node.hash_mask.is_bit_set(self.nibble as u8),
            },
            None => false,
        }
    }

    /// Returns the root hash of the current node, if it has one.
    pub fn hash(&self) -> Option<H256> {
        if self.hash_flag() {
            let node = self.node.as_ref().unwrap();
            match self.nibble {
                -1 => node.root_hash,
                _ => Some(node.hash_for_nibble(self.nibble as u8)),
            }
        } else {
            None
        }
    }
}
