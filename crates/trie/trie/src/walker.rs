use crate::{
    prefix_set::PrefixSet,
    trie_cursor::{subnode::SubNodePosition, CursorSubNode, TrieCursor},
    BranchNodeCompact, Nibbles,
};
use alloy_primitives::{map::HashSet, B256};
use reth_storage_errors::db::DatabaseError;
use tracing::{instrument, trace};

#[cfg(feature = "metrics")]
use crate::metrics::WalkerMetrics;

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
    #[cfg(feature = "metrics")]
    /// Walker metrics.
    metrics: WalkerMetrics,
}

impl<C> TrieWalker<C> {
    /// Constructs a new `TrieWalker` for the state trie from existing stack and a cursor.
    pub fn state_trie_from_stack(cursor: C, stack: Vec<CursorSubNode>, changes: PrefixSet) -> Self {
        Self::from_stack(
            cursor,
            stack,
            changes,
            #[cfg(feature = "metrics")]
            crate::TrieType::State,
        )
    }

    /// Constructs a new `TrieWalker` for the storage trie from existing stack and a cursor.
    pub fn storage_trie_from_stack(
        cursor: C,
        stack: Vec<CursorSubNode>,
        changes: PrefixSet,
    ) -> Self {
        Self::from_stack(
            cursor,
            stack,
            changes,
            #[cfg(feature = "metrics")]
            crate::TrieType::Storage,
        )
    }

    /// Constructs a new `TrieWalker` from existing stack and a cursor.
    fn from_stack(
        cursor: C,
        stack: Vec<CursorSubNode>,
        changes: PrefixSet,
        #[cfg(feature = "metrics")] trie_type: crate::TrieType,
    ) -> Self {
        let mut this = Self {
            cursor,
            changes,
            stack,
            can_skip_current_node: false,
            removed_keys: None,
            #[cfg(feature = "metrics")]
            metrics: WalkerMetrics::new(trie_type),
        };
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
        let keys = self.take_removed_keys();
        (self.stack, keys)
    }

    /// Take removed keys from the walker.
    pub fn take_removed_keys(&mut self) -> HashSet<Nibbles> {
        self.removed_keys.take().unwrap_or_default()
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

    /// Returns the current hash in the trie, if any.
    pub fn hash(&self) -> Option<B256> {
        self.stack.last().and_then(|n| n.hash())
    }

    /// Returns the current hash in the trie, if any.
    ///
    /// Differs from [`Self::hash`] in that it returns `None` if the subnode is positioned at the
    /// child without a hash mask bit set. [`Self::hash`] panics in that case.
    pub fn maybe_hash(&self) -> Option<B256> {
        self.stack.last().and_then(|n| n.maybe_hash())
    }

    /// Indicates whether the children of the current node are present in the trie.
    pub fn children_are_in_trie(&self) -> bool {
        self.stack.last().is_some_and(|n| n.tree_flag())
    }

    /// Returns the next unprocessed key in the trie along with its raw [`Nibbles`] representation.
    #[instrument(level = "trace", skip(self), ret)]
    pub fn next_unprocessed_key(&self) -> Option<(B256, Nibbles)> {
        self.key()
            .and_then(
                |key| if self.can_skip_current_node { key.increment() } else { Some(key.clone()) },
            )
            .map(|key| {
                let mut packed = key.pack();
                packed.resize(32, 0);
                (B256::from_slice(packed.as_slice()), key)
            })
    }

    /// Updates the skip node flag based on the walker's current state.
    fn update_skip_node(&mut self) {
        let old = self.can_skip_current_node;
        self.can_skip_current_node = self
            .stack
            .last()
            .is_some_and(|node| !self.changes.contains(node.full_key()) && node.hash_flag());
        trace!(
            target: "trie::walker",
            old,
            new = self.can_skip_current_node,
            last = ?self.stack.last(),
            "updated skip node flag"
        );
    }
}

impl<C: TrieCursor> TrieWalker<C> {
    /// Constructs a new [`TrieWalker`] for the state trie.
    pub fn state_trie(cursor: C, changes: PrefixSet) -> Self {
        Self::new(
            cursor,
            changes,
            #[cfg(feature = "metrics")]
            crate::TrieType::State,
        )
    }

    /// Constructs a new [`TrieWalker`] for the storage trie.
    pub fn storage_trie(cursor: C, changes: PrefixSet) -> Self {
        Self::new(
            cursor,
            changes,
            #[cfg(feature = "metrics")]
            crate::TrieType::Storage,
        )
    }

    /// Constructs a new `TrieWalker`, setting up the initial state of the stack and cursor.
    fn new(
        cursor: C,
        changes: PrefixSet,
        #[cfg(feature = "metrics")] trie_type: crate::TrieType,
    ) -> Self {
        // Initialize the walker with a single empty stack element.
        let mut this = Self {
            cursor,
            changes,
            stack: vec![CursorSubNode::default()],
            can_skip_current_node: false,
            removed_keys: None,
            #[cfg(feature = "metrics")]
            metrics: WalkerMetrics::new(trie_type),
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
    /// The new key can then be obtained via `key()`.
    ///
    /// # Returns
    ///
    /// * `Result<(), Error>` - Unit on success or an error.
    pub fn advance(&mut self) -> Result<(), DatabaseError> {
        if let Some(last) = self.stack.last() {
            if !self.can_skip_current_node && self.children_are_in_trie() {
                trace!(
                    target: "trie::walker",
                    position = ?last.position(),
                    "cannot skip current node and children are in the trie"
                );
                // If we can't skip the current node and the children are in the trie,
                // either consume the next node or move to the next sibling.
                match last.position() {
                    SubNodePosition::ParentBranch => self.move_to_next_sibling(true)?,
                    SubNodePosition::Child(_) => self.consume_node()?,
                }
            } else {
                trace!(target: "trie::walker", "can skip current node");
                // If we can skip the current node, move to the next sibling.
                self.move_to_next_sibling(false)?;
            }

            // Update the skip node flag based on the new position in the trie.
            self.update_skip_node();
        }

        Ok(())
    }

    /// Retrieves the current root node from the DB, seeking either the exact node or the next one.
    fn node(&mut self, exact: bool) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let key = self.key().expect("key must exist").clone();
        let entry = if exact { self.cursor.seek_exact(key)? } else { self.cursor.seek(key)? };
        #[cfg(feature = "metrics")]
        self.metrics.inc_branch_nodes_seeked();

        if let Some((_, node)) = &entry {
            assert!(!node.state_mask.is_empty());
        }

        Ok(entry)
    }

    /// Consumes the next node in the trie, updating the stack.
    #[instrument(level = "trace", skip(self), ret)]
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
            self.stack[0].set_nibble(key[0]);
        }

        // The current tree mask might have been set incorrectly.
        // Sanity check that the newly retrieved trie node key is the child of the last item
        // on the stack. If not, advance to the next sibling instead of adding the node to the
        // stack.
        if let Some(subnode) = self.stack.last() {
            if !key.starts_with(subnode.full_key()) {
                #[cfg(feature = "metrics")]
                self.metrics.inc_out_of_order_subnode(1);
                self.move_to_next_sibling(false)?;
                return Ok(())
            }
        }

        // Create a new CursorSubNode and push it to the stack.
        let subnode = CursorSubNode::new(key, Some(node));
        let position = subnode.position();
        self.stack.push(subnode);
        self.update_skip_node();

        // Delete the current node if it's included in the prefix set or it doesn't contain the root
        // hash.
        if !self.can_skip_current_node || position.is_child() {
            if let Some((keys, key)) = self.removed_keys.as_mut().zip(self.cursor.current()?) {
                keys.insert(key);
            }
        }

        Ok(())
    }

    /// Moves to the next sibling node in the trie, updating the stack.
    #[instrument(level = "trace", skip(self), ret)]
    fn move_to_next_sibling(
        &mut self,
        allow_root_to_child_nibble: bool,
    ) -> Result<(), DatabaseError> {
        let Some(subnode) = self.stack.last_mut() else { return Ok(()) };

        // Check if the walker needs to backtrack to the previous level in the trie during its
        // traversal.
        if subnode.position().is_last_child() ||
            (subnode.position().is_parent() && !allow_root_to_child_nibble)
        {
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
            let position = subnode.position();
            if subnode.state_flag() {
                trace!(target: "trie::walker", ?position, "found next sibling with state");
                return Ok(())
            }
            if position.is_last_child() {
                trace!(target: "trie::walker", ?position, "checked all siblings");
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
