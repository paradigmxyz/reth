use crate::{BranchNodeCompact, Nibbles, StoredSubNode, CHILD_INDEX_RANGE};
use alloy_primitives::B256;

/// Cursor for iterating over a subtrie.
#[derive(Clone)]
pub struct CursorSubNode {
    /// The key of the current node.
    pub key: Nibbles,
    /// The index of the next child to visit.
    nibble: Nibble,
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
            .field("nibble", &self.nibble)
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
        let nibble = value.nibble.map_or(Nibble::Parent, Nibble::Child);
        let key = Nibbles::from_nibbles_unchecked(value.key);
        let full_key = full_key(key.clone(), nibble);
        Self { key, nibble, node: value.node, full_key }
    }
}

impl From<CursorSubNode> for StoredSubNode {
    fn from(value: CursorSubNode) -> Self {
        Self { key: value.key.to_vec(), nibble: value.nibble.as_child(), node: value.node }
    }
}

impl CursorSubNode {
    /// Creates a new `CursorSubNode` from a key and an optional node.
    pub fn new(key: Nibbles, node: Option<BranchNodeCompact>) -> Self {
        // Find the first nibble that is set in the state mask of the node.
        let nibble = node.as_ref().filter(|n| n.root_hash.is_none()).map_or(Nibble::Parent, |n| {
            Nibble::Child(CHILD_INDEX_RANGE.clone().find(|i| n.state_mask.is_bit_set(*i)).unwrap())
        });
        let full_key = full_key(key.clone(), nibble);
        Self { key, node, nibble, full_key }
    }

    /// Returns the full key of the current node.
    #[inline]
    pub const fn full_key(&self) -> &Nibbles {
        &self.full_key
    }

    /// Returns `true` if the state flag is set for the current nibble.
    #[inline]
    pub fn state_flag(&self) -> bool {
        self.node.as_ref().is_none_or(|node| {
            self.nibble.as_child().is_none_or(|nibble| node.state_mask.is_bit_set(nibble))
        })
    }

    /// Returns `true` if the tree flag is set for the current nibble.
    #[inline]
    pub fn tree_flag(&self) -> bool {
        self.node.as_ref().is_none_or(|node| {
            self.nibble.as_child().is_none_or(|nibble| node.tree_mask.is_bit_set(nibble))
        })
    }

    /// Returns `true` if the current nibble has a root hash.
    pub fn hash_flag(&self) -> bool {
        self.node.as_ref().is_some_and(|node| match self.nibble {
            // This guy has it
            Nibble::Parent => node.root_hash.is_some(),
            // Or get it from the children
            Nibble::Child(nibble) => node.hash_mask.is_bit_set(nibble),
        })
    }

    /// Returns the root hash of the current node, if it has one.
    pub fn hash(&self) -> Option<B256> {
        if self.hash_flag() {
            let node = self.node.as_ref().unwrap();
            match self.nibble {
                Nibble::Parent => node.root_hash,
                Nibble::Child(nibble) => Some(node.hash_for_nibble(nibble)),
            }
        } else {
            None
        }
    }

    /// Returns the next child index to visit.
    #[inline]
    pub const fn nibble(&self) -> Nibble {
        self.nibble
    }

    /// Increments the nibble index.
    #[inline]
    pub fn inc_nibble(&mut self) {
        let old_nibble = self.nibble;
        self.nibble.increment();
        update_full_key(&mut self.full_key, old_nibble, self.nibble);
    }

    /// Sets the nibble index.
    #[inline]
    pub fn set_nibble(&mut self, nibble: u8) {
        let old_nibble = self.nibble;
        self.nibble = Nibble::Child(nibble);
        update_full_key(&mut self.full_key, old_nibble, self.nibble);
    }
}

/// Represents the nibble index of the current node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Nibble {
    /// No nibble is set, pointing to the parent node that the children are contained in.
    Parent,
    /// A nibble is set, pointing to a child node.
    Child(u8),
}

impl Nibble {
    /// Returns `true` if the nibble is set to the parent node.
    pub fn is_parent(&self) -> bool {
        matches!(self, Self::Parent)
    }

    /// Returns `true` if the nibble is set to a child node.
    pub fn is_child(&self) -> bool {
        matches!(self, Self::Child(_))
    }

    /// Returns the child nibble if the nibble is set to a child node.
    pub fn as_child(&self) -> Option<u8> {
        match self {
            Self::Child(nibble) => Some(*nibble),
            _ => None,
        }
    }

    fn increment(&mut self) {
        match self {
            Self::Parent => *self = Self::Child(0),
            Self::Child(nibble) => *nibble += 1,
        }
    }
}

/// Constructs a full key from the given `Nibbles` and `nibble`.
#[inline]
fn full_key(mut key: Nibbles, nibble: Nibble) -> Nibbles {
    if let Some(nibble) = nibble.as_child() {
        key.push(nibble);
    }
    key
}

/// Updates the key by replacing or appending a nibble based on the old and new nibble values.
#[inline]
fn update_full_key(key: &mut Nibbles, old_nibble: Nibble, new_nibble: Nibble) {
    if let Some(new_nibble) = new_nibble.as_child() {
        if old_nibble.is_child() {
            let last_index = key.len() - 1;
            key.set_at(last_index, new_nibble);
        } else {
            key.push(new_nibble);
        }
    } else if old_nibble.is_child() {
        key.pop();
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn nibble_order() {
        let nibbles = [
            Nibble::Parent,
            Nibble::Child(0),
            Nibble::Child(1),
            Nibble::Child(2),
            Nibble::Child(3),
            Nibble::Child(4),
            Nibble::Child(5),
            Nibble::Child(6),
            Nibble::Child(7),
            Nibble::Child(8),
            Nibble::Child(9),
            Nibble::Child(10),
            Nibble::Child(11),
            Nibble::Child(12),
            Nibble::Child(13),
            Nibble::Child(14),
            Nibble::Child(15),
        ];
        assert!(nibbles.is_sorted());
    }
}
