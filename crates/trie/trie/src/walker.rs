use crate::{
    prefix_set::PrefixSet,
    trie_cursor::{subnode::SubNodePosition, CursorSubNode, TrieCursor},
    BranchNodeCompact, Nibbles,
};
use alloy_primitives::{map::HashSet, B256};
use alloy_trie::proof::AddedRemovedKeys;
use reth_storage_errors::db::DatabaseError;
use tracing::{instrument, trace};

#[cfg(feature = "metrics")]
use crate::metrics::WalkerMetrics;

/// Traverses the trie in lexicographic order.
///
/// This iterator depends on the ordering guarantees of [`TrieCursor`].
#[derive(Debug)]
pub struct TrieWalker<C, K = AddedRemovedKeys> {
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
    /// When enabled, all descendants of a branch become unskippable if the branch path itself
    /// matches the prefix set, even if a given descendant path does not.
    walk_all_changed_branch_children: bool,
    /// Lookahead from the ordered trie cursor, reused for subsequent forward seeks.
    seeked_node: Option<SeekedTrieNode>,
    /// The retained trie node keys that need to be removed.
    removed_keys: Option<HashSet<Nibbles>>,
    /// Provided when it's necessary not to skip certain nodes during proof generation.
    /// Specifically we don't skip certain branch nodes even when they are not in the `PrefixSet`,
    /// when they might be required to support leaf removal.
    added_removed_keys: Option<K>,
    #[cfg(feature = "metrics")]
    /// Walker metrics.
    metrics: WalkerMetrics,
}

impl<C: TrieCursor, K: AsRef<AddedRemovedKeys>> TrieWalker<C, K> {
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
            walk_all_changed_branch_children: false,
            seeked_node: None,
            removed_keys: None,
            added_removed_keys: None,
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

    /// Configures the walker to not skip certain branch nodes, even when they are not in the
    /// `PrefixSet`, when they might be needed to support leaf removal.
    pub fn with_added_removed_keys<K2>(self, added_removed_keys: Option<K2>) -> TrieWalker<C, K2> {
        TrieWalker {
            cursor: self.cursor,
            stack: self.stack,
            can_skip_current_node: self.can_skip_current_node,
            changes: self.changes,
            walk_all_changed_branch_children: self.walk_all_changed_branch_children,
            seeked_node: self.seeked_node,
            removed_keys: self.removed_keys,
            added_removed_keys,
            #[cfg(feature = "metrics")]
            metrics: self.metrics,
        }
    }

    /// Configures the walker to treat every descendant of a matching branch path as unskippable.
    pub fn with_walk_all_changed_branch_children(mut self, enabled: bool) -> Self {
        self.walk_all_changed_branch_children = enabled;
        self.update_skip_node();
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

    /// Indicates whether the ordered cursor contains a cached branch at or beneath the current
    /// child prefix, independently of the stored tree mask.
    pub fn children_are_in_trie(&mut self) -> Result<bool, DatabaseError> {
        let Some(subnode) = self.stack.last() else { return Ok(false) };
        if subnode.position().is_parent() {
            return Ok(subnode.node.is_some())
        }
        let prefix = *subnode.full_key();
        Ok(self.seek_node(prefix)?.is_some_and(|(key, _)| key.starts_with(&prefix)))
    }

    /// Returns the next unprocessed key in the trie along with its raw [`Nibbles`] representation.
    #[instrument(level = "trace", skip(self), ret)]
    pub fn next_unprocessed_key(&self) -> Option<(B256, Nibbles)> {
        self.key()
            .and_then(|key| if self.can_skip_current_node { key.increment() } else { Some(*key) })
            .map(|key| (B256::right_padding_from(&key.pack()), key))
    }

    /// Updates the skip node flag based on the walker's current state.
    fn update_skip_node(&mut self) {
        let old = self.can_skip_current_node;
        let forced_walk = self.is_forced_walk();
        self.can_skip_current_node = self.stack.last().is_some_and(|node| {
            // If the current key is not removed according to the [`AddedRemovedKeys`], and all of
            // its siblings are removed, then we don't want to skip it. This allows the
            // `ProofRetainer` to include this node in the returned proofs. Required to support
            // leaf removal.
            let key_is_only_nonremoved_child =
                self.added_removed_keys.as_ref().is_some_and(|added_removed_keys| {
                    node.full_key_is_only_nonremoved_child(added_removed_keys.as_ref())
                });

            trace!(
                target: "trie::walker",
                ?key_is_only_nonremoved_child,
                full_key=?node.full_key(),
                "Checked for only non-removed child",
            );

            !self.changes.contains(node.full_key()) &&
                !forced_walk &&
                node.hash_flag() &&
                !key_is_only_nonremoved_child
        });
        trace!(
            target: "trie::walker",
            old,
            new = self.can_skip_current_node,
            last = ?self.stack.last(),
            "updated skip node flag"
        );
    }

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
            walk_all_changed_branch_children: false,
            seeked_node: None,
            removed_keys: None,
            added_removed_keys: Default::default(),
            #[cfg(feature = "metrics")]
            metrics: WalkerMetrics::new(trie_type),
        };

        // Set up the root node of the trie in the stack, if it exists.
        if let Some((key, value)) = this.node(true).unwrap() {
            this.stack[0] = this.cursor_subnode(key, value).unwrap();
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
            if self.can_skip_current_node {
                trace!(target: "trie::walker", "can skip current node");
                // If we can skip the current node, move to the next sibling.
                self.move_to_next_sibling(false)?;
            } else {
                trace!(
                    target: "trie::walker",
                    position = ?last.position(),
                    "cannot skip current node"
                );
                // Discover descendants directly from the ordered cursor, even when their
                // parent does not advertise them in its tree mask.
                match last.position() {
                    SubNodePosition::ParentBranch => self.move_to_next_sibling(true)?,
                    SubNodePosition::Child(_) => self.consume_node()?,
                }
            }

            // Update the skip node flag based on the new position in the trie.
            self.update_skip_node();
        }

        Ok(())
    }

    /// Retrieves the current root node from the DB, seeking either the exact node or the next one.
    fn node(&mut self, exact: bool) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let key = self.key().expect("key must exist");
        let entry = if exact {
            let entry = self.cursor.seek_exact(*key)?;
            #[cfg(feature = "metrics")]
            self.metrics.inc_branch_nodes_seeked();
            entry
        } else {
            self.seek_node(*key)?
        };

        if let Some((_, node)) = &entry {
            assert!(!node.state_mask.is_empty());
        }

        Ok(entry)
    }

    /// Consumes the next node in the trie, updating the stack.
    #[instrument(level = "trace", skip(self), ret)]
    fn consume_node(&mut self) -> Result<(), DatabaseError> {
        let Some((key, node)) = self.node(false)? else {
            // There may still be cached hashes in siblings even when there are no more stored
            // descendants. Preserve the stack so those hashes can be reused.
            return self.move_to_next_sibling(false)
        };

        // Overwrite the root node's first nibble
        // We need to sync the stack with the trie structure when consuming a new node. This is
        // necessary for proper traversal and accurately representing the trie in the stack.
        if !key.is_empty() && self.stack.len() == 1 && self.stack[0].node.is_none() {
            self.stack[0].set_nibble(key.get_unchecked(0));
        }

        // A seek can return a node from a later sibling. Only consume nodes within the current
        // child prefix; retain the lookahead for the sibling which owns it.
        if let Some(subnode) = self.stack.last() &&
            !key.starts_with(subnode.full_key())
        {
            #[cfg(feature = "metrics")]
            self.metrics.inc_out_of_order_subnode(1);
            self.move_to_next_sibling(false)?;
            return Ok(())
        }

        // Create a new CursorSubNode and push it to the stack.
        let subnode = self.cursor_subnode(key, node)?;
        let position = subnode.position();
        self.stack.push(subnode);
        self.update_skip_node();

        // Delete the current node if it's included in the prefix set or it doesn't contain the root
        // hash.
        if (!self.can_skip_current_node || position.is_child()) &&
            let Some(keys) = self.removed_keys.as_mut()
        {
            keys.insert(key);
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

        // Merge state-mask children with physically stored descendants. In particular, cached
        // branches in state-mask gaps must still be visited and removed during recovery.
        loop {
            if self.stack.last().is_some_and(|node| node.state_flag()) ||
                self.children_are_in_trie()?
            {
                return Ok(())
            }
            let subnode = self.stack.last_mut().expect("current subnode exists");
            if subnode.position().is_last_child() {
                break
            }
            subnode.inc_nibble();
        }

        // Pop the current node and move to the next sibling.
        self.stack.pop();
        self.move_to_next_sibling(false)?;

        Ok(())
    }

    /// Seeks stored branches directly instead of trusting a parent's tree mask.
    ///
    /// For example, parent `0x3` can have tree-mask bit `c` clear while the database still holds
    /// `0x3c` and `0x3c4...`. Seeking child prefix `0x3c` discovers those entries regardless of
    /// the mask. The caller checks the returned path's prefix, since a seek can also return a
    /// later sibling. Recovery walks the discovered descendants without reusing their stale
    /// hashes, records their keys for deletion, and lets the root calculation regenerate any
    /// branches which should remain.
    ///
    /// Cache lookahead so empty child prefixes before the next stored branch, and repeated
    /// checks of the same prefix, do not each require another database seek.
    fn seek_node(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if let Some(seeked) = &self.seeked_node &&
            seeked.key <= key &&
            seeked.entry.as_ref().is_none_or(|(path, _)| *path >= key)
        {
            return Ok(seeked.entry.clone())
        }

        let entry = self.cursor.seek(key)?;
        #[cfg(feature = "metrics")]
        self.metrics.inc_branch_nodes_seeked();
        self.seeked_node = Some(SeekedTrieNode { key, entry: entry.clone() });
        Ok(entry)
    }

    /// Keeps every descendant of a forcibly walked child unskippable. This is derived from the
    /// ancestor stack, including after checkpoint resumption, so adopting an orphan branch cannot
    /// introduce stale hashes from its children into the root calculation.
    fn is_forced_walk(&mut self) -> bool {
        self.walk_all_changed_branch_children &&
            self.stack.iter().any(|node| {
                node.node.is_some() &&
                    node.position().is_child() &&
                    self.changes.contains(&node.key)
            })
    }

    /// Starts a cached branch at the earliest child with state or stored descendants. A stale
    /// state mask must not hide a stored child preceding its first set bit.
    fn cursor_subnode(
        &mut self,
        key: Nibbles,
        node: BranchNodeCompact,
    ) -> Result<CursorSubNode, DatabaseError> {
        let mut subnode = CursorSubNode::new(key, Some(node));
        if let SubNodePosition::Child(first) = subnode.position() &&
            first > 0
        {
            let mut prefix = key;
            prefix.push(0);
            if let Some((path, _)) = self.seek_node(prefix)? &&
                path.starts_with(&key)
            {
                subnode.set_nibble(first.min(path.get_unchecked(key.len())));
            }
        }
        Ok(subnode)
    }
}

/// Lookahead from a seek in the lexicographically ordered trie cursor.
#[derive(Debug)]
struct SeekedTrieNode {
    /// The prefix passed to the cursor.
    key: Nibbles,
    /// The first stored node at or after that prefix, or `None` when exhausted.
    entry: Option<(Nibbles, BranchNodeCompact)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        prefix_set::PrefixSetMut,
        progress::StorageRootProgress,
        test_utils::{storage_root_prehashed, TrieTestHarness},
        trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
        updates::StorageTrieUpdates,
        StorageRoot,
    };
    use alloy_primitives::{map::B256Map, B256, U256};
    use alloy_trie::TrieMask;
    use std::collections::BTreeMap;

    fn branch_node(state_mask: u16, tree_mask: u16, hash_mask: u16) -> BranchNodeCompact {
        let hash_count = hash_mask.count_ones() as usize;
        BranchNodeCompact::new(
            TrieMask::new(state_mask),
            TrieMask::new(tree_mask),
            TrieMask::new(hash_mask),
            vec![B256::ZERO; hash_count],
            None,
        )
    }

    fn root_branch_node(state_mask: u16, tree_mask: u16, hash_mask: u16) -> BranchNodeCompact {
        let hash_count = hash_mask.count_ones() as usize;
        BranchNodeCompact::new(
            TrieMask::new(state_mask),
            TrieMask::new(tree_mask),
            TrieMask::new(hash_mask),
            vec![B256::ZERO; hash_count],
            Some(B256::ZERO),
        )
    }

    fn walker_for_matching_branch_children_test(
        walk_all_changed_branch_children: bool,
    ) -> TrieWalker<crate::trie_cursor::mock::MockTrieCursor> {
        let trie_nodes = BTreeMap::from([
            (Nibbles::default(), root_branch_node(1 << 2, 1 << 2, 1 << 2)),
            (
                Nibbles::from_nibbles([0x2]),
                branch_node((1 << 3) | (1 << 4), 0, (1 << 3) | (1 << 4)),
            ),
        ]);
        let factory = MockTrieCursorFactory::new(trie_nodes, B256Map::default());

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(Nibbles::from_nibbles([0x2, 0x3, 0x1]));

        TrieWalker::state_trie(factory.account_trie_cursor().unwrap(), prefix_set.freeze())
            .with_walk_all_changed_branch_children(walk_all_changed_branch_children)
    }

    #[test]
    fn branch_siblings_remain_skippable_by_default() {
        let mut walker = walker_for_matching_branch_children_test(false);

        assert_eq!(walker.key().copied(), Some(Nibbles::default()));
        assert!(!walker.can_skip_current_node);

        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x2])));
        assert!(!walker.can_skip_current_node);

        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x2, 0x3])));
        assert_eq!(walker.stack.last().unwrap().position(), SubNodePosition::Child(0x3));
        assert!(!walker.can_skip_current_node);

        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x2, 0x4])));
        assert!(walker.can_skip_current_node);
    }

    #[test]
    fn matching_branch_path_can_make_all_children_unskippable() {
        let mut walker = walker_for_matching_branch_children_test(true);

        walker.advance().unwrap();
        walker.advance().unwrap();
        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x2, 0x4])));
        assert!(!walker.can_skip_current_node);
    }

    #[test]
    fn changed_branch_children_remove_orphans() {
        for regenerate_branch in [false, true] {
            let mut storage = BTreeMap::from([
                (B256::right_padding_from(&[0x3a]), U256::ONE),
                (B256::right_padding_from(&[0x3a, 0x01]), U256::from(10)),
                (B256::right_padding_from(&[0x3a, 0x10]), U256::from(7)),
                (B256::right_padding_from(&[0x3b]), U256::from(2)),
                (B256::right_padding_from(&[0x3c, 0x47]), U256::from(3)),
                (B256::right_padding_from(&[0x80]), U256::from(4)),
                (B256::right_padding_from(&[0x81]), U256::from(5)),
                (B256::right_padding_from(&[0x80, 0x10]), U256::from(8)),
            ]);
            if regenerate_branch {
                storage.insert(B256::right_padding_from(&[0x3c, 0x48]), U256::from(6));
                storage.insert(B256::right_padding_from(&[0x3c, 0x47, 0x10]), U256::from(9));
            }
            let mut harness = TrieTestHarness::new(storage.clone());
            let expected_nodes = harness
                .storage_trie_updates()
                .storage_nodes
                .iter()
                .map(|(key, node)| (*key, node.clone()))
                .collect::<BTreeMap<_, _>>();
            let mut stored_nodes = expected_nodes.clone();
            let parent = Nibbles::from_nibbles([0x3]);
            stored_nodes.get_mut(&parent).unwrap().tree_mask &=
                !TrieMask::new((1 << 0xa) | (1 << 0xc));

            // The cached branch at 0x3c and its descendants are unreachable through 0x3's mask.
            // Include a descendant which should be regenerated and one which should disappear.
            let orphan_paths = [
                Nibbles::from_nibbles([0x3, 0xc]),
                Nibbles::from_nibbles([0x3, 0xc, 0x4]),
                Nibbles::from_nibbles([0x3, 0xc, 0x4, 0xf]),
                Nibbles::from_nibbles([0x3, 0xf, 0x1]),
            ];
            // The valid branch under 0x3a is also hidden by the mask. Its regenerated update can
            // be split off before the walker finishes 0x3, so resumption must not delete it again.
            assert!(expected_nodes.contains_key(&Nibbles::from_nibbles([0x3, 0xa])));
            for path in orphan_paths {
                let mut stale_node = branch_node(0b11, 0, 0b11);
                stale_node.hashes = vec![B256::repeat_byte(0xee); 2].into();
                stored_nodes.insert(path, stale_node);
            }
            harness.set_trie_nodes(stored_nodes.clone());

            // A change under sibling 0x3a must also clean up 0x3c and state-mask gaps.
            let prefix_set =
                PrefixSetMut::from([Nibbles::unpack(B256::right_padding_from(&[0x3a]))]).freeze();
            for (enabled, threshold) in [(false, u64::MAX), (true, u64::MAX), (true, 1)] {
                // Force leaf processing throughout the checkpointed calculation, including the
                // neighboring subtree, so the low threshold splits the regenerated updates.
                let changes =
                    if threshold == 1 { PrefixSet::all_paths() } else { prefix_set.clone() };
                let (root, updates) =
                    storage_root_with_progress(&harness, changes, enabled, threshold);

                let mut resulting_nodes = stored_nodes.clone();
                for (path, node) in updates.into_sorted().storage_nodes {
                    if let Some(node) = node {
                        resulting_nodes.insert(path, node);
                    } else {
                        resulting_nodes.remove(&path);
                    }
                }
                if enabled {
                    assert_eq!(root, storage_root_prehashed(storage.clone()));
                    assert_eq!(resulting_nodes, expected_nodes);
                } else {
                    // Normal traversal trusts unchanged cached hashes. Recovery must override
                    // that behavior all the way through a forcibly walked orphan subtree.
                    assert_ne!(root, storage_root_prehashed(storage.clone()));
                    assert_ne!(resulting_nodes, expected_nodes);
                }
            }
        }
    }

    #[test]
    fn initial_root_removes_orphans() {
        let orphan = Nibbles::from_nibbles([0xf, 0x1]);
        let factory = MockTrieCursorFactory::new(
            BTreeMap::from([
                (Nibbles::default(), root_branch_node(0b11, 0, 0b11)),
                (orphan, branch_node(0b11, 0, 0b11)),
            ]),
            B256Map::default(),
        );
        let changes = PrefixSetMut::from([Nibbles::from_nibbles([0x0, 0x1])]).freeze();
        let mut walker =
            TrieWalker::<_>::state_trie(factory.account_trie_cursor().unwrap(), changes)
                .with_walk_all_changed_branch_children(true)
                .with_deletions_retained(true);

        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x0])));
        while walker.key().is_some() {
            walker.advance().unwrap();
        }
        assert_eq!(walker.take_removed_keys(), HashSet::from_iter([orphan]));
    }

    fn storage_root_with_progress(
        harness: &TrieTestHarness,
        changes: PrefixSet,
        enabled: bool,
        threshold: u64,
    ) -> (B256, StorageTrieUpdates) {
        let mut previous_state = None;
        let mut updates = StorageTrieUpdates::default();
        let mut checkpoints = 0;
        loop {
            let progress = StorageRoot::new_hashed(
                harness.trie_cursor_factory(),
                harness.hashed_cursor_factory(),
                harness.hashed_address(),
                changes.clone(),
                #[cfg(feature = "metrics")]
                crate::metrics::TrieRootMetrics::new(crate::TrieType::Storage),
            )
            .with_walk_all_changed_branch_children(enabled)
            .with_threshold(threshold)
            .with_intermediate_state(previous_state)
            .root_with_progress()
            .unwrap();
            match progress {
                StorageRootProgress::Progress(state, _, new_updates) => {
                    updates.extend(new_updates);
                    previous_state = Some(*state);
                    checkpoints += 1;
                }
                StorageRootProgress::Complete(root, _, new_updates) => {
                    updates.extend(new_updates);
                    if threshold == 1 {
                        assert!(checkpoints > 1);
                    }
                    return (root, updates)
                }
            }
        }
    }

    #[test]
    fn normal_walk_derives_cached_descendants_from_cursor() {
        let changed_slot = B256::right_padding_from(&[0x33, 0x50]);
        let storage = BTreeMap::from([
            (B256::right_padding_from(&[0x33, 0x40]), U256::ONE),
            (B256::right_padding_from(&[0x33, 0x40, 0x10]), U256::ONE),
            (B256::right_padding_from(&[0x33, 0x41]), U256::ONE),
            (changed_slot, U256::ONE),
            (B256::right_padding_from(&[0x33, 0x51]), U256::ONE),
        ]);
        let old_state = TrieTestHarness::new(storage.clone());
        let mut stored_nodes = old_state
            .storage_trie_updates()
            .storage_nodes
            .iter()
            .map(|(path, node)| (*path, node.clone()))
            .collect::<BTreeMap<_, _>>();
        let parent = Nibbles::from_nibbles([0x3, 0x3]);
        let child = Nibbles::from_nibbles([0x3, 0x3, 0x4]);
        assert!(stored_nodes.contains_key(&child));
        stored_nodes.get_mut(&parent).unwrap().tree_mask = TrieMask::default();

        let mut new_storage = storage;
        new_storage.insert(changed_slot, U256::from(7));
        let mut harness = TrieTestHarness::new(new_storage.clone());
        let expected_parent = harness.storage_trie_updates().storage_nodes[&parent].clone();
        harness.set_trie_nodes(stored_nodes);
        let changes = PrefixSetMut::from([Nibbles::unpack(changed_slot)]).freeze();
        let (root, updates) = storage_root_with_progress(&harness, changes, false, u64::MAX);

        assert_eq!(root, storage_root_prehashed(new_storage));
        // Child 4's hash is reused, but the regenerated parent must still point to its stored
        // descendants even though the old tree-mask bit was clear.
        assert_eq!(updates.storage_nodes[&parent], expected_parent);
    }

    #[test]
    fn stored_child_before_first_state_bit_is_walked() {
        let parent = Nibbles::from_nibbles([0x3]);
        let orphan = Nibbles::from_nibbles([0x3, 0x1, 0x4]);
        let neighbor = Nibbles::from_nibbles([0x4]);
        let factory = MockTrieCursorFactory::new(
            BTreeMap::from([
                (parent, branch_node(1 << 0xa, 0, 0)),
                (orphan, branch_node(0b11, 0, 0b11)),
                (neighbor, branch_node(0b11, 0, 0b11)),
            ]),
            B256Map::default(),
        );
        let changes = PrefixSetMut::from([Nibbles::from_nibbles([0x3, 0xa])]).freeze();
        let mut walker =
            TrieWalker::<_>::state_trie(factory.account_trie_cursor().unwrap(), changes)
                .with_walk_all_changed_branch_children(true)
                .with_deletions_retained(true);

        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x3, 0x1])));
        walker.advance().unwrap();
        assert_eq!(walker.key().copied(), Some(Nibbles::from_nibbles([0x3, 0x1, 0x4, 0x0])));
        assert!(!walker.can_skip_current_node);
        while walker.key().is_some() {
            walker.advance().unwrap();
        }
        // The later neighboring node is consumed only after leaving the orphan's subtree.
        assert_eq!(walker.take_removed_keys(), HashSet::from_iter([parent, orphan, neighbor]));
    }
}
