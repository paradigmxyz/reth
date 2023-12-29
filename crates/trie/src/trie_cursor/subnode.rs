use reth_primitives::{
    trie::{nodes::CHILD_INDEX_RANGE, BranchNodeCompact, Nibbles, StoredSubNode},
    B256,
};

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
        let nibble = match value.nibble {
            Some(n) => n as i8,
            None => -1,
        };
        let key = Nibbles::from_nibbles_unchecked(value.key);
        let full_key = full_key(key.clone(), nibble);
        Self { key, nibble, node: value.node, full_key }
    }
}

impl From<CursorSubNode> for StoredSubNode {
    fn from(value: CursorSubNode) -> Self {
        let nibble = if value.nibble >= 0 { Some(value.nibble as u8) } else { None };
        Self { key: value.key.to_vec(), nibble, node: value.node }
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
        let full_key = full_key(key.clone(), nibble);
        CursorSubNode { key, node, nibble, full_key }
    }

    /// Returns the full key of the current node.
    pub fn full_key(&self) -> &Nibbles {
        &self.full_key
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
    pub fn hash(&self) -> Option<B256> {
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

    /// Returns the next child index to visit.
    #[inline]
    pub fn nibble(&self) -> i8 {
        self.nibble
    }

    /// Increments the nibble index.
    #[inline]
    pub fn inc_nibble(&mut self) {
        self.nibble += 1;
        update_full_key(&mut self.full_key, self.nibble - 1, self.nibble);
    }

    /// Sets the nibble index.
    #[inline]
    pub fn set_nibble(&mut self, nibble: i8) {
        let old_nibble = self.nibble;
        self.nibble = nibble;
        update_full_key(&mut self.full_key, old_nibble, self.nibble);
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

/// Updates the key by replacing or appending a nibble based on the old and new nibble values.
#[inline]
fn update_full_key(key: &mut Nibbles, old_nibble: i8, new_nibble: i8) {
    if new_nibble >= 0 {
        if old_nibble >= 0 {
            let last_index = key.len() - 1;
            key.set_at(last_index, new_nibble as u8);
        } else {
            key.push(new_nibble as u8);
        }
    } else if old_nibble >= 0 {
        key.pop();
    }
}
