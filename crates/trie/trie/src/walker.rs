use crate::{
    prefix_set::PrefixSet,
    trie_cursor::{CursorSubNode, TrieCursor},
    BranchNodeCompact, Nibbles,
};
use alloy_primitives::B256;
use reth_storage_errors::db::DatabaseError;
use std::collections::HashSet;

/// `TrieWalker` is a structure that enables traversal of a Merkle trie.
/// It allows moving through the trie in a depth-first manner, skipping certain branches
/// if they have not changed.
#[derive(Debug)]
pub struct TrieWalker<C> {
    /// A mutable reference to a trie cursor instance used for navigating the trie.
    pub cursor: C,
    /// A vector containing the trie nodes that have been visited.
    pub stack: Vec<CursorSubNode>,
    /// A flag indicating whether the current node can be skipped when traversing the trie. This
    /// is determined by whether the current key's prefix is included in the prefix set and if the
    /// hash flag is set.
    pub can_skip_current_node: bool,
    /// A `PrefixSet` representing the changes to be applied to the trie.
    pub changes: PrefixSet,
    /// The retained trie node keys that need to be removed.
    removed_keys: Option<HashSet<Nibbles>>,
}

impl<C> TrieWalker<C> {
    /// Constructs a new `TrieWalker` from existing stack and a cursor.
    pub fn from_stack(cursor: C, stack: Vec<CursorSubNode>, changes: PrefixSet) -> Self {
        let mut this =
            Self { cursor, changes, stack, can_skip_current_node: false, removed_keys: None };
        this.update_skip_node();
        this
    }

    /// Sets the flag whether the trie updates should be stored.
    pub fn with_deletions_retained(mut self, retained: bool) -> Self {
        if retained {
            self.removed_keys = Some(HashSet::default());
        }
        self
    }

    /// Split the walker into stack and trie updates.
    pub fn split(mut self) -> (Vec<CursorSubNode>, HashSet<Nibbles>) {
        let keys = self.removed_keys.take();
        (self.stack, keys.unwrap_or_default())
    }

    /// Prints the current stack of trie nodes.
    pub fn print_stack(&self) {
        println!("====================== STACK ======================");
        for node in &self.stack {
            println!("{node:?}");
        }
        println!("====================== END STACK ======================\n");
    }

    /// The current length of the removed keys.
    pub fn removed_keys_len(&self) -> usize {
        self.removed_keys.as_ref().map_or(0, |u| u.len())
    }

    /// Returns the current key in the trie.
    pub fn key(&self) -> Option<&Nibbles> {
        self.stack.last().map(|n| n.full_key())
    }

    /// Returns the current hash in the trie if any.
    pub fn hash(&self) -> Option<B256> {
        self.stack.last().and_then(|n| n.hash())
    }

    /// Indicates whether the children of the current node are present in the trie.
    pub fn children_are_in_trie(&self) -> bool {
        self.stack.last().map_or(false, |n| n.tree_flag())
    }

    /// Returns the next unprocessed key in the trie.
    pub fn next_unprocessed_key(&self) -> Option<B256> {
        self.key()
            .and_then(|key| {
                if self.can_skip_current_node {
                    key.increment().map(|inc| inc.pack())
                } else {
                    Some(key.pack())
                }
            })
            .map(|mut key| {
                key.resize(32, 0);
                B256::from_slice(key.as_slice())
            })
    }

    /// Updates the skip node flag based on the walker's current state.
    fn update_skip_node(&mut self) {
        self.can_skip_current_node = self
            .stack
            .last()
            .map_or(false, |node| !self.changes.contains(node.full_key()) && node.hash_flag());
    }
}

impl<C: TrieCursor> TrieWalker<C> {
    /// Constructs a new `TrieWalker`, setting up the initial state of the stack and cursor.
    pub fn new(cursor: C, changes: PrefixSet) -> Self {
        // Initialize the walker with a single empty stack element.
        let mut this = Self {
            cursor,
            changes,
            stack: vec![CursorSubNode::default()],
            can_skip_current_node: false,
            removed_keys: None,
        };

        // Set up the root node of the trie in the stack, if it exists.
        if let Some((key, value)) = this.node(true).unwrap() {
            this.stack[0] = CursorSubNode::new(key, Some(value));
        }

        // Update the skip state for the root node.
        this.update_skip_node();
        this
    }

    /// Advances the walker to the next trie node and updates the skip node flag.
    ///
    /// # Returns
    ///
    /// * `Result<Option<Nibbles>, Error>` - The next key in the trie or an error.
    pub fn advance(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        if let Some(last) = self.stack.last() {
            if !self.can_skip_current_node && self.children_are_in_trie() {
                // If we can't skip the current node and the children are in the trie,
                // either consume the next node or move to the next sibling.
                match last.nibble() {
                    -1 => self.move_to_next_sibling(true)?,
                    _ => self.consume_node()?,
                }
            } else {
                // If we can skip the current node, move to the next sibling.
                self.move_to_next_sibling(false)?;
            }

            // Update the skip node flag based on the new position in the trie.
            self.update_skip_node();
        }

        // Return the current key.
        Ok(self.key().cloned())
    }

    /// Retrieves the current root node from the DB, seeking either the exact node or the next one.
    fn node(&mut self, exact: bool) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let key = self.key().expect("key must exist").clone();
        let entry = if exact { self.cursor.seek_exact(key)? } else { self.cursor.seek(key)? };

        if let Some((_, node)) = &entry {
            assert!(!node.state_mask.is_empty());
        }

        Ok(entry)
    }

    /// Consumes the next node in the trie, updating the stack.
    fn consume_node(&mut self) -> Result<(), DatabaseError> {
        let Some((key, node)) = self.node(false)? else {
            // If no next node is found, clear the stack.
            self.stack.clear();
            return Ok(())
        };

        // Overwrite the root node's first nibble
        // We need to sync the stack with the trie structure when consuming a new node. This is
        // necessary for proper traversal and accurately representing the trie in the stack.
        if !key.is_empty() && !self.stack.is_empty() {
            self.stack[0].set_nibble(key[0] as i8);
        }

        // Create a new CursorSubNode and push it to the stack.
        let subnode = CursorSubNode::new(key, Some(node));
        let nibble = subnode.nibble();
        self.stack.push(subnode);
        self.update_skip_node();

        // Delete the current node if it's included in the prefix set or it doesn't contain the root
        // hash.
        if !self.can_skip_current_node || nibble != -1 {
            if let Some((keys, key)) = self.removed_keys.as_mut().zip(self.cursor.current()?) {
                keys.insert(key);
            }
        }

        Ok(())
    }

    /// Moves to the next sibling node in the trie, updating the stack.
    fn move_to_next_sibling(
        &mut self,
        allow_root_to_child_nibble: bool,
    ) -> Result<(), DatabaseError> {
        let Some(subnode) = self.stack.last_mut() else { return Ok(()) };

        // Check if the walker needs to backtrack to the previous level in the trie during its
        // traversal.
        if subnode.nibble() >= 0xf || (subnode.nibble() < 0 && !allow_root_to_child_nibble) {
            self.stack.pop();
            self.move_to_next_sibling(false)?;
            return Ok(())
        }

        subnode.inc_nibble();

        if subnode.node.is_none() {
            return self.consume_node()
        }

        // Find the next sibling with state.
        loop {
            if subnode.state_flag() {
                return Ok(())
            }
            if subnode.nibble() == 0xf {
                break
            }
            subnode.inc_nibble();
        }

        // Pop the current node and move to the next sibling.
        self.stack.pop();
        self.move_to_next_sibling(false)?;

        Ok(())
    }
}
