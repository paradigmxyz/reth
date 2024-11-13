use crate::{BranchNodeCompact, Nibbles, StoredSubNode, CHILD_INDEX_RANGE};
use alloy_primitives::B256;

/// Cursor for iterating over a subtrie.
#[derive(Clone)]
pub struct CursorSubNode {
    /// The key of the current node.
    pub key: Nibbles,
    /// The index of the next child to visit.
    nibble: i8,
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
        let nibble = value.nibble.map_or(-1, |n| n as i8);
        let key = Nibbles::from_nibbles_unchecked(value.key);
        let full_key = full_key(key.clone(), nibble);
        Self { key, nibble, node: value.node, full_key }
    }
}

impl From<CursorSubNode> for StoredSubNode {
    fn from(value: CursorSubNode) -> Self {
        let nibble = (value.nibble >= 0).then_some(value.nibble as u8);
        Self { key: value.key.to_vec(), nibble, node: value.node }
    }
}

impl CursorSubNode {
    /// Creates a new `CursorSubNode` from a key and an optional node.
    pub fn new(key: Nibbles, node: Option<BranchNodeCompact>) -> Self {
        // Fast path for finding initial nibble
        let nibble = if let Some(node) = node.as_ref() {
            if node.root_hash.is_some() {
                -1
            } else {
                // Find first set bit in state mask
                CHILD_INDEX_RANGE
                    .clone()
                    .find(|&i| node.state_mask.is_bit_set(i))
                    .map_or(-1, |i| i as i8)
            }
        } else {
            -1
        };

        Self { key: key.clone(), nibble, node, full_key: Self::compute_full_key(key, nibble) }
    }

    /// Efficient full key computation
    #[inline]
    fn compute_full_key(mut key: Nibbles, nibble: i8) -> Nibbles {
        if nibble >= 0 {
            key.push(nibble as u8);
        }
        key
    }

    /// Returns the full key of the current node.
    #[inline]
    pub const fn full_key(&self) -> &Nibbles {
        &self.full_key
    }

    /// Returns true if the state flag is set for the current nibble.
    #[inline]
    pub const fn state_flag(&self) -> bool {
        match (self.node.as_ref(), self.nibble) {
            (None, _) | (_, -1) => true,
            (Some(node), nibble) => node.state_mask.is_bit_set(nibble as u8),
        }
    }

    /// Returns `true` if the tree flag is set for the current nibble.
    #[inline]
    pub const fn tree_flag(&self) -> bool {
        match (self.node.as_ref(), self.nibble) {
            (None, _) | (_, -1) => true,
            (Some(node), nibble) => node.tree_mask.is_bit_set(nibble as u8),
        }
    }
    /// Returns `true` if the state flag is set for the current nibble.
    #[inline]
    pub const fn hash_flag(&self) -> bool {
        match (self.node.as_ref(), self.nibble) {
            (Some(node), -1) => node.root_hash.is_some(),
            (Some(node), n) => node.hash_mask.is_bit_set(n as u8),
            (None, _) => false,
        }
    }

    /// Increments the nibble index.
    #[inline]
    pub fn inc_nibble(&mut self) {
        let old_nibble = self.nibble;
        self.nibble += 1;
        self.update_full_key(old_nibble);
    }

    /// Returns the root hash of the current node, if it has one.
    #[inline]
    pub fn hash(&self) -> Option<B256> {
        self.node.as_ref().and_then(|node| match self.nibble {
            -1 => node.root_hash,
            n if node.hash_mask.is_bit_set(n as u8) => Some(node.hash_for_nibble(n as u8)),
            _ => None,
        })
    }

    /// Returns the next child index to visit.
    #[inline]
    pub const fn nibble(&self) -> i8 {
        self.nibble
    }

    /// Sets the nibble index Increments the nibble index.
    #[inline]
    pub fn set_nibble(&mut self, new_nibble: i8) {
        let old_nibble = self.nibble;
        self.nibble = new_nibble;
        self.update_full_key(old_nibble);
    }

    /// Updates the key by replacing or appending a nibble based on the old and new nibble values.
    #[inline]
    fn update_full_key(&mut self, old_nibble: i8) {
        match (old_nibble >= 0, self.nibble >= 0) {
            (true, true) => {
                let last_index = self.full_key.len() - 1;
                self.full_key.set_at(last_index, self.nibble as u8);
            }
            (false, true) => self.full_key.push(self.nibble as u8),
            (true, false) => {
                self.full_key.pop();
            }
            (false, false) => (), 
        }
    }
}

/// Constructs a full key from the given `Nibbles` and `nibble`.
#[inline]
fn full_key(mut key: Nibbles, nibble: i8) -> Nibbles {
    if nibble >= 0 {
        key.push(nibble as u8);
    }
    key
}
