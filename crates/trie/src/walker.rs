use crate::{
    prefix_set::PrefixSet,
    trie_cursor::{CursorSubNode, TrieCursor},
    updates::TrieUpdates,
};
use reth_db::DatabaseError;
use reth_primitives::{
    trie::{BranchNodeCompact, Nibbles},
    B256,
};

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
    /// The trie updates to be applied to the trie.
    trie_updates: Option<TrieUpdates>,
}

impl<C: TrieCursor> TrieWalker<C> {
    /// Constructs a new TrieWalker, setting up the initial state of the stack and cursor.
    pub fn new(cursor: C, changes: PrefixSet) -> Self {
        // Initialize the walker with a single empty stack element.
        let mut this = Self {
            cursor,
            changes,
            stack: vec![CursorSubNode::default()],
            can_skip_current_node: false,
            trie_updates: None,
        };

        // Set up the root node of the trie in the stack, if it exists.
        if let Some((key, value)) = this.node(true).unwrap() {
            this.stack[0] = CursorSubNode::new(key, Some(value));
        }

        // Update the skip state for the root node.
        this.update_skip_node();
        this
    }

    /// Constructs a new TrieWalker from existing stack and a cursor.
    pub fn from_stack(cursor: C, stack: Vec<CursorSubNode>, changes: PrefixSet) -> Self {
        let mut this =
            Self { cursor, changes, stack, can_skip_current_node: false, trie_updates: None };
        this.update_skip_node();
        this
    }

    /// Sets the flag whether the trie updates should be stored.
    pub fn with_updates(mut self, retain_updates: bool) -> Self {
        self.set_updates(retain_updates);
        self
    }

    /// Sets the flag whether the trie updates should be stored.
    pub fn set_updates(&mut self, retain_updates: bool) {
        if retain_updates {
            self.trie_updates = Some(TrieUpdates::default());
        }
    }

    /// Split the walker into stack and trie updates.
    pub fn split(mut self) -> (Vec<CursorSubNode>, TrieUpdates) {
        let trie_updates = self.trie_updates.take();
        (self.stack, trie_updates.unwrap_or_default())
    }

    /// Prints the current stack of trie nodes.
    pub fn print_stack(&self) {
        println!("====================== STACK ======================");
        for node in &self.stack {
            println!("{node:?}");
        }
        println!("====================== END STACK ======================\n");
    }

    /// The current length of the trie updates.
    pub fn updates_len(&self) -> usize {
        self.trie_updates.as_ref().map(|u| u.len()).unwrap_or(0)
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
        let key = self.key().expect("key must exist");
        let entry = if exact {
            self.cursor.seek_exact(key.to_vec().into())?
        } else {
            self.cursor.seek(key.to_vec().into())?
        };

        if let Some((_, node)) = &entry {
            assert!(!node.state_mask.is_empty());
        }

        Ok(entry.map(|(k, v)| (Nibbles::from_nibbles_unchecked(k), v)))
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
            if let Some((updates, key)) = self.trie_updates.as_mut().zip(self.cursor.current()?) {
                updates.schedule_delete(key);
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
        if subnode.nibble() >= 15 || (subnode.nibble() < 0 && !allow_root_to_child_nibble) {
            self.stack.pop();
            self.move_to_next_sibling(false)?;
            return Ok(())
        }

        subnode.inc_nibble();

        if subnode.node.is_none() {
            return self.consume_node()
        }

        // Find the next sibling with state.
        while subnode.nibble() < 16 {
            if subnode.state_flag() {
                return Ok(())
            }
            subnode.inc_nibble();
        }

        // Pop the current node and move to the next sibling.
        self.stack.pop();
        self.move_to_next_sibling(false)?;

        Ok(())
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
        self.can_skip_current_node = if let Some(node) = self.stack.last() {
            !self.changes.contains(node.full_key()) && node.hash_flag()
        } else {
            false
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        prefix_set::PrefixSetMut,
        trie_cursor::{DatabaseAccountTrieCursor, DatabaseStorageTrieCursor},
    };
    use reth_db::{cursor::DbCursorRW, tables, transaction::DbTxMut};
    use reth_primitives::trie::{StorageTrieEntry, StoredBranchNode};
    use reth_provider::test_utils::create_test_provider_factory;

    #[test]
    fn walk_nodes_with_common_prefix() {
        let inputs = vec![
            (vec![0x5u8], BranchNodeCompact::new(0b1_0000_0101, 0b1_0000_0100, 0, vec![], None)),
            (vec![0x5u8, 0x2, 0xC], BranchNodeCompact::new(0b1000_0111, 0, 0, vec![], None)),
            (vec![0x5u8, 0x8], BranchNodeCompact::new(0b0110, 0b0100, 0, vec![], None)),
        ];
        let expected = vec![
            vec![0x5, 0x0],
            // The [0x5, 0x2] prefix is shared by the first 2 nodes, however:
            // 1. 0x2 for the first node points to the child node path
            // 2. 0x2 for the second node is a key.
            // So to proceed to add 1 and 3, we need to push the sibling first (0xC).
            vec![0x5, 0x2],
            vec![0x5, 0x2, 0xC, 0x0],
            vec![0x5, 0x2, 0xC, 0x1],
            vec![0x5, 0x2, 0xC, 0x2],
            vec![0x5, 0x2, 0xC, 0x7],
            vec![0x5, 0x8],
            vec![0x5, 0x8, 0x1],
            vec![0x5, 0x8, 0x2],
        ];

        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap();

        let mut account_cursor = tx.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();
        for (k, v) in &inputs {
            account_cursor.upsert(k.clone().into(), StoredBranchNode(v.clone())).unwrap();
        }
        let account_trie = DatabaseAccountTrieCursor::new(account_cursor);
        test_cursor(account_trie, &expected);

        let hashed_address = B256::random();
        let mut storage_cursor = tx.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
        for (k, v) in &inputs {
            storage_cursor
                .upsert(
                    hashed_address,
                    StorageTrieEntry { nibbles: k.clone().into(), node: v.clone() },
                )
                .unwrap();
        }
        let storage_trie = DatabaseStorageTrieCursor::new(storage_cursor, hashed_address);
        test_cursor(storage_trie, &expected);
    }

    fn test_cursor<T>(mut trie: T, expected: &[Vec<u8>])
    where
        T: TrieCursor,
    {
        let mut walker = TrieWalker::new(&mut trie, Default::default());
        assert!(walker.key().unwrap().is_empty());

        // We're traversing the path in lexigraphical order.
        for expected in expected {
            let got = walker.advance().unwrap();
            assert_eq!(got.unwrap(), Nibbles::from_nibbles_unchecked(expected.clone()));
        }

        // There should be 8 paths traversed in total from 3 branches.
        let got = walker.advance().unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn cursor_rootnode_with_changesets() {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap();
        let mut cursor = tx.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

        let nodes = vec![
            (
                vec![],
                BranchNodeCompact::new(
                    // 2 and 4 are set
                    0b10100,
                    0b00100,
                    0,
                    vec![],
                    Some(B256::random()),
                ),
            ),
            (
                vec![0x2],
                BranchNodeCompact::new(
                    // 1 is set
                    0b00010,
                    0,
                    0b00010,
                    vec![B256::random()],
                    None,
                ),
            ),
        ];

        let hashed_address = B256::random();
        for (k, v) in nodes {
            cursor.upsert(hashed_address, StorageTrieEntry { nibbles: k.into(), node: v }).unwrap();
        }

        let mut trie = DatabaseStorageTrieCursor::new(cursor, hashed_address);

        // No changes
        let mut cursor = TrieWalker::new(&mut trie, Default::default());
        assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles_unchecked([]))); // root
        assert!(cursor.can_skip_current_node); // due to root_hash
        cursor.advance().unwrap(); // skips to the end of trie
        assert_eq!(cursor.key().cloned(), None);

        // We insert something that's not part of the existing trie/prefix.
        let mut changed = PrefixSetMut::default();
        changed.insert(Nibbles::from_nibbles_unchecked([0xF, 0x1]));
        let mut cursor = TrieWalker::new(&mut trie, changed.freeze());

        // Root node
        assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles_unchecked([])));
        // Should not be able to skip state due to the changed values
        assert!(!cursor.can_skip_current_node);
        cursor.advance().unwrap();
        assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles_unchecked([0x2])));
        cursor.advance().unwrap();
        assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles_unchecked([0x2, 0x1])));
        cursor.advance().unwrap();
        assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles_unchecked([0x4])));

        cursor.advance().unwrap();
        assert_eq!(cursor.key().cloned(), None); // the end of trie
    }
}
