use crate::{BranchNodeCompact, Nibbles, StoredSubNode, CHILD_INDEX_RANGE};
use alloy_primitives::B256;

/// Cursor for iterating over a subtrie.
#[derive(Clone)]
pub struct CursorSubNode {
    /// The key of the current node.
    pub key: Nibbles,
    /// The position of the current subnode.
    position: SubNodePosition,
    /// The node itself.
    pub node: Option<BranchNodeCompact>,
    /// Full key
    full_key: Nibbles,
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
            .field("position", &self.position)
            .field("state_flag", &self.state_flag())
            .field("tree_flag", &self.tree_flag())
            .field("hash_flag", &self.hash_flag())
            .field("hash", &self.hash())
            .finish()
    }
}

/// Implements conversion from `StoredSubNode` to `CursorSubNode`.
impl From<StoredSubNode> for CursorSubNode {
    /// Converts a `StoredSubNode` into a `CursorSubNode`.
    ///
    /// Extracts necessary values from the `StoredSubNode` and constructs
    /// a corresponding `CursorSubNode`.
    fn from(value: StoredSubNode) -> Self {
        let position = value.nibble.map_or(SubNodePosition::ParentBranch, SubNodePosition::Child);
        let key = Nibbles::from_nibbles_unchecked(value.key);
        Self::new_with_full_key(key, value.node, position)
    }
}

impl From<CursorSubNode> for StoredSubNode {
    fn from(value: CursorSubNode) -> Self {
        Self { key: value.key.to_vec(), nibble: value.position.as_child(), node: value.node }
    }
}

impl CursorSubNode {
    /// Creates a new [`CursorSubNode`] from a key and an optional node.
    pub fn new(key: Nibbles, node: Option<BranchNodeCompact>) -> Self {
        // Find the first nibble that is set in the state mask of the node.
        let position = node.as_ref().filter(|n| n.root_hash.is_none()).map_or(
            SubNodePosition::ParentBranch,
            |n| {
                SubNodePosition::Child(
                    CHILD_INDEX_RANGE.clone().find(|i| n.state_mask.is_bit_set(*i)).unwrap(),
                )
            },
        );
        Self::new_with_full_key(key, node, position)
    }

    /// Creates a new [`CursorSubNode`] and sets the full key according to the provided key and
    /// position.
    fn new_with_full_key(
        key: Nibbles,
        node: Option<BranchNodeCompact>,
        position: SubNodePosition,
    ) -> Self {
        let mut full_key = key.clone();
        if let Some(nibble) = position.as_child() {
            full_key.push(nibble);
        }
        Self { key, node, position, full_key }
    }

    /// Returns the full key of the current node.
    #[inline]
    pub const fn full_key(&self) -> &Nibbles {
        &self.full_key
    }

    /// Updates the full key by replacing or appending a child nibble based on the old subnode
    /// position.
    #[inline]
    fn update_full_key(&mut self, old_position: SubNodePosition) {
        if let Some(new_nibble) = self.position.as_child() {
            if old_position.is_child() {
                let last_index = self.full_key.len() - 1;
                self.full_key.set_at(last_index, new_nibble);
            } else {
                self.full_key.push(new_nibble);
            }
        } else if old_position.is_child() {
            self.full_key.pop();
        }
    }

    /// Returns `true` if either of these:
    /// - No current node is set.
    /// - The current node is a parent branch node.
    /// - The current node is a child with state mask bit set in the parent branch node.
    #[inline]
    pub fn state_flag(&self) -> bool {
        self.node.as_ref().is_none_or(|node| {
            self.position.as_child().is_none_or(|nibble| node.state_mask.is_bit_set(nibble))
        })
    }

    /// Returns `true` if either of these:
    /// - No current node is set.
    /// - The current node is a parent branch node.
    /// - The current node is a child with tree mask bit set in the parent branch node.
    #[inline]
    pub fn tree_flag(&self) -> bool {
        self.node.as_ref().is_none_or(|node| {
            self.position.as_child().is_none_or(|nibble| node.tree_mask.is_bit_set(nibble))
        })
    }

    /// Returns `true` if the hash for the current node is set.
    ///
    /// It means either of two:
    /// - Current node is a parent branch node, and it has a root hash.
    /// - Current node is a child node, and it has a hash mask bit set in the parent branch node.
    pub fn hash_flag(&self) -> bool {
        self.node.as_ref().is_some_and(|node| match self.position {
            // Check if the parent branch node has a root hash
            SubNodePosition::ParentBranch => node.root_hash.is_some(),
            // Or get it from the children
            SubNodePosition::Child(nibble) => node.hash_mask.is_bit_set(nibble),
        })
    }

    /// Returns the hash of the current node.
    ///
    /// It means either of two:
    /// - Root hash of the parent branch node.
    /// - Hash of the child node at the current nibble, if it has a hash mask bit set in the parent
    ///   branch node.
    pub fn hash(&self) -> Option<B256> {
        self.node.as_ref().and_then(|node| match self.position {
            // Get the root hash for the parent branch node
            SubNodePosition::ParentBranch => node.root_hash,
            // Or get it from the children
            SubNodePosition::Child(nibble) => Some(node.hash_for_nibble(nibble)),
        })
    }

    /// Returns the position to the current node.
    #[inline]
    pub const fn position(&self) -> SubNodePosition {
        self.position
    }

    /// Increments the nibble index.
    #[inline]
    pub fn inc_nibble(&mut self) {
        let old_position = self.position;
        self.position.increment();
        self.update_full_key(old_position);
    }

    /// Sets the nibble index.
    #[inline]
    pub fn set_nibble(&mut self, nibble: u8) {
        let old_position = self.position;
        self.position = SubNodePosition::Child(nibble);
        self.update_full_key(old_position);
    }
}

/// Represents a subnode position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubNodePosition {
    /// Positioned at the parent branch node.
    ParentBranch,
    /// Positioned at a child node at the given nibble.
    Child(u8),
}

impl SubNodePosition {
    /// Returns `true` if the position is set to the parent branch node.
    pub const fn is_parent(&self) -> bool {
        matches!(self, Self::ParentBranch)
    }

    /// Returns `true` if the position is set to a child node.
    pub const fn is_child(&self) -> bool {
        matches!(self, Self::Child(_))
    }

    /// Returns the nibble of the child node if the position is set to a child node.
    pub const fn as_child(&self) -> Option<u8> {
        match self {
            Self::Child(nibble) => Some(*nibble),
            _ => None,
        }
    }

    /// Returns `true` if the position is set to a last child nibble (i.e. greater than or equal to
    /// 0xf).
    pub const fn is_last_child(&self) -> bool {
        match self {
            Self::ParentBranch => false,
            Self::Child(nibble) => *nibble >= 0xf,
        }
    }

    const fn increment(&mut self) {
        match self {
            Self::ParentBranch => *self = Self::Child(0),
            Self::Child(nibble) => *nibble += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnode_position_ord() {
        assert!([
            SubNodePosition::ParentBranch,
            SubNodePosition::Child(0),
            SubNodePosition::Child(1),
            SubNodePosition::Child(2),
            SubNodePosition::Child(3),
            SubNodePosition::Child(4),
            SubNodePosition::Child(5),
            SubNodePosition::Child(6),
            SubNodePosition::Child(7),
            SubNodePosition::Child(8),
            SubNodePosition::Child(9),
            SubNodePosition::Child(10),
            SubNodePosition::Child(11),
            SubNodePosition::Child(12),
            SubNodePosition::Child(13),
            SubNodePosition::Child(14),
            SubNodePosition::Child(15),
        ]
        .is_sorted());
    }

    #[test]
    fn subnode_position_is_last_child() {
        assert!([
            SubNodePosition::ParentBranch,
            SubNodePosition::Child(0),
            SubNodePosition::Child(1),
            SubNodePosition::Child(2),
            SubNodePosition::Child(3),
            SubNodePosition::Child(4),
            SubNodePosition::Child(5),
            SubNodePosition::Child(6),
            SubNodePosition::Child(7),
            SubNodePosition::Child(8),
            SubNodePosition::Child(9),
            SubNodePosition::Child(10),
            SubNodePosition::Child(11),
            SubNodePosition::Child(12),
            SubNodePosition::Child(13),
            SubNodePosition::Child(14),
        ]
        .iter()
        .all(|position| !position.is_last_child()));
        assert!(SubNodePosition::Child(15).is_last_child());
        assert!(SubNodePosition::Child(16).is_last_child());
    }
}
