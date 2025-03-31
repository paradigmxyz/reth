use crate::{BranchNodeCompact, Nibbles, StoredSubNode, CHILD_INDEX_RANGE};
use alloy_primitives::B256;

/// Cursor for iterating over a subtrie.
#[derive(Clone)]
pub struct CursorSubNode {
    /// The key of the current node.
    pub key: Nibbles,
    /// The pointer to the current node.
    pointer: NodePointer,
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
            .field("pointer", &self.pointer)
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
        let pointer = value.nibble.map_or(NodePointer::ParentBranch, NodePointer::Child);
        let key = Nibbles::from_nibbles_unchecked(value.key);
        let full_key = full_key(key.clone(), pointer);
        Self { key, pointer, node: value.node, full_key }
    }
}

impl From<CursorSubNode> for StoredSubNode {
    fn from(value: CursorSubNode) -> Self {
        Self { key: value.key.to_vec(), nibble: value.pointer.as_child(), node: value.node }
    }
}

impl CursorSubNode {
    /// Creates a new `CursorSubNode` from a key and an optional node.
    pub fn new(key: Nibbles, node: Option<BranchNodeCompact>) -> Self {
        // Find the first nibble that is set in the state mask of the node.
        let pointer = node.as_ref().filter(|n| n.root_hash.is_none()).map_or(
            NodePointer::ParentBranch,
            |n| {
                NodePointer::Child(
                    CHILD_INDEX_RANGE.clone().find(|i| n.state_mask.is_bit_set(*i)).unwrap(),
                )
            },
        );
        let full_key = full_key(key.clone(), pointer);
        Self { key, node, pointer, full_key }
    }

    /// Returns the full key of the current node.
    #[inline]
    pub const fn full_key(&self) -> &Nibbles {
        &self.full_key
    }

    /// Returns `true` if either of these:
    /// - No current node is set.
    /// - The current node is a parent branch node.
    /// - The current node is a child with state mask bit set in the parent branch node.
    #[inline]
    pub fn state_flag(&self) -> bool {
        self.node.as_ref().is_none_or(|node| {
            self.pointer.as_child().is_none_or(|nibble| node.state_mask.is_bit_set(nibble))
        })
    }

    /// Returns `true` if either of these:
    /// - No current node is set.
    /// - The current node is a parent branch node.
    /// - The current node is a child with tree mask bit set in the parent branch node.
    #[inline]
    pub fn tree_flag(&self) -> bool {
        self.node.as_ref().is_none_or(|node| {
            self.pointer.as_child().is_none_or(|nibble| node.tree_mask.is_bit_set(nibble))
        })
    }

    /// Returns `true` if the hash for the current node is set.
    ///
    /// It means either of two:
    /// - Current node is a parent branch node, and it has a root hash.
    /// - Current node is a child node, and it has a hash mask bit set in the parent branch node.
    pub fn hash_flag(&self) -> bool {
        self.node.as_ref().is_some_and(|node| match self.pointer {
            // Check if the parent branch node has a root hash
            NodePointer::ParentBranch => node.root_hash.is_some(),
            // Or get it from the children
            NodePointer::Child(nibble) => node.hash_mask.is_bit_set(nibble),
        })
    }

    /// Returns the hash of the current node.
    ///
    /// It means either of two:
    /// - Root hash of the parent branch node.
    /// - Hash of the child node at the current nibble, if it has a hash mask bit set in the parent
    ///   branch node.
    pub fn hash(&self) -> Option<B256> {
        self.node.as_ref().and_then(|node| match self.pointer {
            // Get the root hash for the parent branch node
            NodePointer::ParentBranch => node.root_hash,
            // Or get it from the children
            NodePointer::Child(nibble) => Some(node.hash_for_nibble(nibble)),
        })
    }

    /// Returns the pointer to the current node.
    #[inline]
    pub const fn pointer(&self) -> NodePointer {
        self.pointer
    }

    /// Increments the nibble index.
    #[inline]
    pub fn inc_nibble(&mut self) {
        let old_nibble = self.pointer;
        self.pointer.increment();
        update_full_key(&mut self.full_key, old_nibble, self.pointer);
    }

    /// Sets the nibble index.
    #[inline]
    pub fn set_nibble(&mut self, nibble: u8) {
        let old_nibble = self.pointer;
        self.pointer = NodePointer::Child(nibble);
        update_full_key(&mut self.full_key, old_nibble, self.pointer);
    }
}

/// Represents a pointer to a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodePointer {
    /// Pointing to the parent branch node.
    ParentBranch,
    /// Pointing to a child node at the given nibble.
    Child(u8),
}

impl NodePointer {
    /// Returns `true` if the pointer is set to the parent branch node.
    pub fn is_parent(&self) -> bool {
        matches!(self, Self::ParentBranch)
    }

    /// Returns `true` if the pointer is set to a child node.
    pub fn is_child(&self) -> bool {
        matches!(self, Self::Child(_))
    }

    /// Returns the nibble of the child node if the pointer is set to a child node.
    pub fn as_child(&self) -> Option<u8> {
        match self {
            Self::Child(nibble) => Some(*nibble),
            _ => None,
        }
    }

    /// Returns `true` if the pointer is set to a child node that is out of bounds (i.e. greater
    /// than 0xf).
    pub fn is_out_of_bounds(&self) -> bool {
        match self {
            Self::ParentBranch => false,
            Self::Child(nibble) => *nibble >= 0xf,
        }
    }

    fn increment(&mut self) {
        match self {
            Self::ParentBranch => *self = Self::Child(0),
            Self::Child(nibble) => *nibble += 1,
        }
    }
}

/// Constructs a full key from the given [`Nibbles`] and [`NodePointer`].
#[inline]
fn full_key(mut key: Nibbles, node_pointer: NodePointer) -> Nibbles {
    if let Some(nibble) = node_pointer.as_child() {
        key.push(nibble);
    }
    key
}

/// Updates the key by replacing or appending a child nibble based on the old and new node pointer
/// values.
#[inline]
fn update_full_key(key: &mut Nibbles, old_pointer: NodePointer, new_pointer: NodePointer) {
    if let Some(new_nibble) = new_pointer.as_child() {
        if old_pointer.is_child() {
            let last_index = key.len() - 1;
            key.set_at(last_index, new_nibble);
        } else {
            key.push(new_nibble);
        }
    } else if old_pointer.is_child() {
        key.pop();
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn node_pointer() {
        let pointers = [
            NodePointer::ParentBranch,
            NodePointer::Child(0),
            NodePointer::Child(1),
            NodePointer::Child(2),
            NodePointer::Child(3),
            NodePointer::Child(4),
            NodePointer::Child(5),
            NodePointer::Child(6),
            NodePointer::Child(7),
            NodePointer::Child(8),
            NodePointer::Child(9),
            NodePointer::Child(10),
            NodePointer::Child(11),
            NodePointer::Child(12),
            NodePointer::Child(13),
            NodePointer::Child(14),
            NodePointer::Child(15),
        ];
        assert!(pointers.is_sorted());
        assert!(pointers.iter().all(|pointer| !pointer.is_out_of_bounds()));

        assert!(NodePointer::Child(16).is_out_of_bounds());
    }
}
