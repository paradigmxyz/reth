use crate::{
    prefix_set::PrefixSet,
    trie_cursor::{subnode::SubNodePosition, CursorSubNode, TrieCursor},
    BranchNodeCompact, Nibbles,
};
use alloy_primitives::{map::HashSet, B256};
use alloy_trie::{proof::AddedRemovedKeys, TrieMask};
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
    /// When enabled, all children of a branch become unskippable if the branch path itself
    /// matches the prefix set, even if a given child path does not.
    walk_all_changed_branch_children: bool,
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
            removed_keys: self.removed_keys,
            added_removed_keys,
            #[cfg(feature = "metrics")]
            metrics: self.metrics,
        }
    }

    /// Configures the walker to treat every child of a matching branch path as unskippable.
    ///
    /// When deletions are retained, also collects stored descendants hidden by a matching
    /// branch's tree mask or left beside an extension's child. These keys appear in the deletion
    /// set even if traversal never visits them. Root calculations can replace these deletions with
    /// regenerated branch updates.
    pub const fn with_walk_all_changed_branch_children(mut self, enabled: bool) -> Self {
        self.walk_all_changed_branch_children = enabled;
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
            .and_then(|key| if self.can_skip_current_node { key.increment() } else { Some(*key) })
            .map(|key| (B256::right_padding_from(&key.pack()), key))
    }

    /// Updates the skip node flag based on the walker's current state.
    fn update_skip_node(&mut self) {
        let old = self.can_skip_current_node;
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

            let branch_path_matches_prefix_set = self.walk_all_changed_branch_children &&
                node.position().is_child() &&
                self.changes.contains(&node.key);

            !self.changes.contains(node.full_key()) &&
                !branch_path_matches_prefix_set &&
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
    pub fn state_trie(cursor: C, changes: PrefixSet) -> Result<Self, DatabaseError> {
        Self::new(
            cursor,
            changes,
            #[cfg(feature = "metrics")]
            crate::TrieType::State,
        )
    }

    /// Constructs a new [`TrieWalker`] for the storage trie.
    pub fn storage_trie(cursor: C, changes: PrefixSet) -> Result<Self, DatabaseError> {
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
    ) -> Result<Self, DatabaseError> {
        // Initialize the walker with a single empty stack element.
        let mut this = Self {
            cursor,
            changes,
            stack: vec![CursorSubNode::default()],
            can_skip_current_node: false,
            walk_all_changed_branch_children: false,
            removed_keys: None,
            added_removed_keys: Default::default(),
            #[cfg(feature = "metrics")]
            metrics: WalkerMetrics::new(trie_type),
        };

        // Set up the root node of the trie in the stack, if it exists.
        if let Some((key, value)) = this.node(true)? {
            this.stack[0] = CursorSubNode::new(key, Some(value));
        }

        // Update the skip state for the root node.
        this.update_skip_node();
        Ok(this)
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
        let key = self.key().expect("key must exist");
        let entry = if exact { self.cursor.seek_exact(*key)? } else { self.cursor.seek(*key)? };
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

        // Only the top-level cursor follows the first nibble of a newly consumed branch.
        // A deeper seek can leave its child interval when a cached tree bit is stale; changing
        // an ancestor here would skip the subtree containing the seek result.
        if !key.is_empty() && self.stack.len() == 1 {
            self.stack[0].set_nibble(key.get_unchecked(0));
        }

        // The current tree mask might have been set incorrectly.
        // Sanity check that the newly retrieved trie node key is the child of the last item
        // on the stack. If not, advance to the next sibling instead of adding the node to the
        // stack.
        if let Some(subnode) = self.stack.last() &&
            !key.starts_with(subnode.full_key())
        {
            #[cfg(feature = "metrics")]
            self.metrics.inc_out_of_order_subnode(1);
            self.move_to_next_sibling(false)?;
            return Ok(())
        }

        self.remove_orphaned_descendants(
            self.stack.last().map_or(key, |node| *node.full_key()),
            key,
            node.tree_mask,
        )?;

        // Create a new CursorSubNode and push it to the stack.
        let subnode = CursorSubNode::new(key, Some(node));
        let position = subnode.position();
        self.stack.push(subnode);
        self.update_skip_node();

        // Delete the current node if it's included in the prefix set or it doesn't contain the root
        // hash.
        if (!self.can_skip_current_node || position.is_child()) &&
            let Some((keys, key)) = self.removed_keys.as_mut().zip(self.cursor.current()?)
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

    /// Removes stored nodes skipped by masks or extensions during recovery.
    ///
    /// When the first stored branch below a changed child is reached through an extension,
    /// traversal will skip from that branch's subtree to the end of the logical child. Collect
    /// any stale siblings in that gap before advancing, just as missing tree bits require
    /// collecting hidden descendants. Bound each scan so live subtrees remain reusable.
    ///
    /// Regenerated branch updates take precedence over these deletions. Restore the cursor to
    /// the consumed node afterwards so traversal and checkpoint resumption remain unchanged.
    fn remove_orphaned_descendants(
        &mut self,
        child_path: Nibbles,
        key: Nibbles,
        tree_mask: TrieMask,
    ) -> Result<(), DatabaseError> {
        if self.walk_all_changed_branch_children && self.removed_keys.is_some() {
            let mut cursor_moved = if key.len() > child_path.len() &&
                self.changes.contains(&child_path) &&
                let Some(start) = key.next_without_prefix()
            {
                self.remove_nodes_in_range(start, child_path.next_without_prefix())?;
                true
            } else {
                false
            };

            let missing_children = !tree_mask;
            if !missing_children.is_empty() && self.changes.contains(&key) {
                for nibble in missing_children.iter() {
                    let mut prefix = key;
                    prefix.push(nibble);
                    self.remove_nodes_in_range(prefix, prefix.next_without_prefix())?;
                }
                cursor_moved = true;
            }

            if cursor_moved {
                self.cursor.seek_exact(key)?;
                #[cfg(feature = "metrics")]
                self.metrics.inc_branch_nodes_seeked();
            }
        }
        Ok(())
    }

    /// Collects stored paths in `[start, end)` for deletion without trusting their hashes.
    /// A missing end covers the remainder of the trie. Leaves the cursor at the range boundary.
    fn remove_nodes_in_range(
        &mut self,
        start: Nibbles,
        end: Option<Nibbles>,
    ) -> Result<(), DatabaseError> {
        if let Some(removed_keys) = &mut self.removed_keys {
            let mut entry = self.cursor.seek(start)?;
            #[cfg(feature = "metrics")]
            self.metrics.inc_branch_nodes_seeked();
            while let Some((path, _)) = entry &&
                end.is_none_or(|end| path < end)
            {
                removed_keys.insert(path);
                entry = self.cursor.next()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::HashedCursorFactory,
        mock::KeyVisitType,
        prefix_set::PrefixSetMut,
        progress::StorageRootProgress,
        proof_v2::StorageProofCalculator,
        test_utils::{storage_root_prehashed, TrieTestHarness},
        trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
        updates::StorageTrieUpdates,
        ProofV2Target, StorageRoot,
    };
    use alloy_primitives::{map::B256Map, B256, U256};
    use proptest::prelude::*;
    use reth_storage_errors::db::DatabaseErrorInfo;
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
            .unwrap()
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
                Nibbles::from_nibbles([0x3, 0x1, 0x4]),
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
                // Also rebuild the neighboring branch in checkpointed runs. Resuming a skipped
                // branch after a leaf checkpoint currently fails even without orphan cleanup.
                let changes = if threshold == 1 {
                    PrefixSetMut::from([
                        Nibbles::unpack(B256::right_padding_from(&[0x3a])),
                        Nibbles::unpack(B256::right_padding_from(&[0x80])),
                    ])
                    .freeze()
                } else {
                    prefix_set.clone()
                };
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
                assert_eq!(root, storage_root_prehashed(storage.clone()));
                if enabled {
                    assert_eq!(resulting_nodes, expected_nodes);
                } else {
                    // Normal traversal ignores hidden descendants, but leaves them in the cache.
                    assert_ne!(resulting_nodes, expected_nodes);
                }
            }
        }
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
    fn orphan_cleanup_skips_live_subtrees() {
        let parent = Nibbles::from_nibbles([0x3]);
        let tree_mask = TrieMask::from_nibble(0xa);
        let orphans = [
            Nibbles::from_nibbles([0x3, 0xb]),
            Nibbles::from_nibbles([0x3, 0xb, 0x5]),
            Nibbles::from_nibbles([0x3, 0xb, 0x5, 0xf]),
        ];
        let mut nodes = BTreeMap::from([
            (parent, branch_node((1 << 0xa) | (1 << 0xb), tree_mask.get(), 0)),
            (Nibbles::from_nibbles([0x3, 0xa]), branch_node(0b11, 0, 0)),
            (Nibbles::from_nibbles([0x4]), branch_node(0b11, 0, 0)),
        ]);
        for index in 0..128 {
            nodes.insert(
                Nibbles::from_nibbles([0x3, 0xa, index / 16, index % 16]),
                branch_node(0b11, 0, 0),
            );
        }
        nodes.extend(orphans.into_iter().map(|path| (path, branch_node(0b11, 0, 0))));
        let factory = MockTrieCursorFactory::new(nodes, B256Map::default());
        let changes = PrefixSetMut::from([Nibbles::from_nibbles([0x3, 0xa])]).freeze();
        let mut walker =
            TrieWalker::<_>::state_trie(factory.account_trie_cursor().unwrap(), changes)
                .unwrap()
                .with_walk_all_changed_branch_children(true)
                .with_deletions_retained(true);
        walker.cursor.seek_exact(parent).unwrap();
        factory.visited_account_keys().clear();
        walker.remove_orphaned_descendants(parent, parent, tree_mask).unwrap();

        assert_eq!(walker.take_removed_keys(), HashSet::from_iter(orphans));
        assert_eq!(walker.cursor.current().unwrap(), Some(parent));
        let next_count = factory
            .visited_account_keys()
            .iter()
            .filter(|visit| matches!(visit.visit_type, KeyVisitType::Next))
            .count();
        assert!(next_count <= 4, "cleanup walked {next_count} entries for 3 orphan nodes");
    }

    #[test]
    fn state_trie_initial_read_timeout_returns_error() {
        let error =
            TrieWalker::<_>::state_trie(TimedOutTrieCursor, PrefixSet::default()).unwrap_err();
        assert_matches::assert_matches!(error, DatabaseError::Read(info) if info == timeout_error());
    }

    #[test]
    fn storage_trie_initial_read_timeout_returns_error() {
        let error =
            TrieWalker::<_>::storage_trie(TimedOutTrieCursor, PrefixSet::default()).unwrap_err();
        assert_matches::assert_matches!(error, DatabaseError::Read(info) if info == timeout_error());
    }

    fn timeout_error() -> DatabaseErrorInfo {
        DatabaseErrorInfo { message: "read transaction has been timed out".into(), code: -96000 }
    }

    #[derive(Debug)]
    struct TimedOutTrieCursor;

    impl TrieCursor for TimedOutTrieCursor {
        fn seek_exact(
            &mut self,
            key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            assert_eq!(key, Nibbles::default());
            Err(DatabaseError::Read(timeout_error()))
        }

        fn seek(
            &mut self,
            _key: Nibbles,
        ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            unreachable!("initialization only seeks the root exactly")
        }

        fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
            unreachable!("initialization only seeks the root exactly")
        }

        fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
            unreachable!("initialization only seeks the root exactly")
        }

        fn reset(&mut self) {
            unreachable!("initialization only seeks the root exactly")
        }
    }

    /// A stale `tree_mask` bit on a changed branch (here `0xa`, bit `e`, with no `0xae` stored)
    /// makes `consume_node` seek past the branch into the next first-nibble subtree. The root
    /// nibble is overwritten with that subtree's nibble before the out-of-order check rejects the
    /// node, so the walker never visits the stale branch at `0xb` and recovery leaves it in the
    /// table. Later roots computed without a change under `0xb` then hash the stale node.
    #[test]
    fn dangling_tree_bit_does_not_skip_next_root_child() {
        let key = |nibbles: &[u8]| B256::right_padding_from(&Nibbles::from_nibbles(nibbles).pack());
        let storage = BTreeMap::from([
            (key(&[0xa, 0x1, 0x0]), U256::from(1)),
            (key(&[0xa, 0x1, 0x1]), U256::from(2)),
            (key(&[0xa, 0xe]), U256::from(3)),
            (key(&[0xb, 0x2]), U256::from(4)),
            (key(&[0xb, 0x7]), U256::from(5)),
            (key(&[0xc, 0x0]), U256::from(6)),
        ]);
        let mut harness = TrieTestHarness::new(storage.clone());
        let expected_root = storage_root_prehashed(storage);
        let expected_nodes = harness
            .storage_trie_updates()
            .storage_nodes
            .iter()
            .map(|(key, node)| (*key, node.clone()))
            .collect::<BTreeMap<_, _>>();
        let branch_a = Nibbles::from_nibbles([0xa]);
        let branch_b = Nibbles::from_nibbles([0xb]);
        assert!(expected_nodes.contains_key(&branch_a));
        assert!(!expected_nodes.contains_key(&branch_b));

        // What a masked write or an unwind inside the masking window can leave on disk: the
        // parent still claims child `e` is stored, and a branch for `0xb` that no longer exists.
        let mut stored_nodes = expected_nodes.clone();
        stored_nodes.get_mut(&branch_a).unwrap().tree_mask |= TrieMask::from_nibble(0xe);
        let mut stale_b = branch_node((1 << 0x2) | (1 << 0x7), 0, (1 << 0x2) | (1 << 0x7));
        stale_b.hashes = vec![B256::repeat_byte(0xee); 2].into();
        stored_nodes.insert(branch_b, stale_b);
        harness.set_trie_nodes(stored_nodes.clone());

        // Partial state trie recovery: both subtrees changed in the unwound range.
        let changes = PrefixSetMut::from([
            Nibbles::unpack(key(&[0xa, 0xe])),
            Nibbles::unpack(key(&[0xb, 0x2])),
        ])
        .freeze();
        let (root, updates) = storage_root_with_progress(&harness, changes, true, u64::MAX);
        assert_eq!(root, expected_root);

        let mut recovered_nodes = stored_nodes;
        for (path, node) in updates.into_sorted().storage_nodes {
            if let Some(node) = node {
                recovered_nodes.insert(path, node);
            } else {
                recovered_nodes.remove(&path);
            }
        }
        let stale_b_left = recovered_nodes.contains_key(&branch_b);

        // The next root without changes reuses the table.
        harness.set_trie_nodes(recovered_nodes.clone());
        let (next_root, _) =
            storage_root_with_progress(&harness, PrefixSet::default(), false, u64::MAX);

        assert!(
            !stale_b_left && next_root == expected_root,
            "recovery left the stale branch at 0xb: {stale_b_left}; next root {next_root} vs \
             expected {expected_root}"
        );
        assert_eq!(recovered_nodes, expected_nodes);
    }

    /// Masked partial persistence can leave a stale branch beside the live child under the same
    /// child nibble: here `0x5a7` (pre-batch, re-touched by the masking suffix) next to `0x5a2`
    /// (written by the batch), with no branch at `0x5a`. Recovery's `consume_node` takes the first
    /// stored node under `0x5a` as that child (an extension), and the walker never looks for other
    /// stored nodes under `0x5a`, so `0x5a7` is never removed. Proof V2 later adopts it as the next
    /// cached branch under `0x5a` and splices its stale hashes into the proof.
    #[test]
    fn stale_sibling_beside_extension_child_is_removed() {
        let key = |nibbles: &[u8]| B256::right_padding_from(&Nibbles::from_nibbles(nibbles).pack());
        for regenerate_sibling in [false, true] {
            let mut storage = BTreeMap::from([
                (key(&[0x5, 0x3]), U256::from(1)),
                (key(&[0x5, 0xa, 0x2, 0x1, 0x0]), U256::from(2)),
                (key(&[0x5, 0xa, 0x2, 0x1, 0x1]), U256::from(3)),
                (key(&[0x5, 0xa, 0x2, 0x2]), U256::from(4)),
                (key(&[0xc, 0x0]), U256::from(5)),
            ]);
            if regenerate_sibling {
                storage.extend([
                    (key(&[0x5, 0xa, 0x7, 0x1, 0x0]), U256::from(6)),
                    (key(&[0x5, 0xa, 0x7, 0x1, 0x1]), U256::from(7)),
                    (key(&[0x5, 0xa, 0x7, 0x2]), U256::from(8)),
                ]);
            }
            let mut harness = TrieTestHarness::new(storage.clone());
            let expected_root = storage_root_prehashed(storage);
            let expected_nodes = harness
                .storage_trie_updates()
                .storage_nodes
                .iter()
                .map(|(key, node)| (*key, node.clone()))
                .collect::<BTreeMap<_, _>>();
            let live = Nibbles::from_nibbles([0x5, 0xa, 0x2]);
            let stale = Nibbles::from_nibbles([0x5, 0xa, 0x7]);
            assert!(expected_nodes.contains_key(&Nibbles::from_nibbles([0x5])));
            assert!(expected_nodes.contains_key(&live));
            assert_eq!(
                expected_nodes.contains_key(&Nibbles::from_nibbles([0x5, 0xa])),
                regenerate_sibling,
            );

            let mut stored_nodes = expected_nodes.clone();
            stored_nodes.remove(&Nibbles::from_nibbles([0x5, 0xa]));
            let mut stale_node = branch_node((1 << 0x1) | (1 << 0x2), 0, 1 << 0x1);
            stale_node.hashes = vec![B256::repeat_byte(0xee)].into();
            stored_nodes.insert(stale, stale_node);
            for threshold in [u64::MAX, 1] {
                harness.set_trie_nodes(stored_nodes.clone());

                // Partial state trie recovery: the masking suffix recreated keys under the stale
                // branch.
                let changes = PrefixSetMut::from([
                    Nibbles::unpack(key(&[0x5, 0xa, 0x7, 0x1, 0x0])),
                    Nibbles::unpack(key(&[0x5, 0xa, 0x7, 0x1, 0x1])),
                    Nibbles::unpack(key(&[0x5, 0xa, 0x7, 0x2])),
                ])
                .freeze();
                let (root, updates) =
                    storage_root_with_progress(&harness, changes, true, threshold);
                assert_eq!(root, expected_root);

                let mut recovered_nodes = stored_nodes.clone();
                for (path, node) in updates.into_sorted().storage_nodes {
                    if let Some(node) = node {
                        recovered_nodes.insert(path, node);
                    } else {
                        recovered_nodes.remove(&path);
                    }
                }

                // Proof V2 over the recovered table, no prefix set, one target under the live
                // child.
                harness.set_trie_nodes(recovered_nodes.clone());
                let tcf = harness.trie_cursor_factory();
                let hcf = harness.hashed_cursor_factory();
                let mut calc = StorageProofCalculator::new_storage(
                    tcf.storage_trie_cursor(harness.hashed_address()).unwrap(),
                    hcf.hashed_storage_cursor(harness.hashed_address()).unwrap(),
                );
                let mut targets = vec![ProofV2Target::new(key(&[0x5, 0xa, 0x2, 0x2]))];
                let nodes = calc.storage_proof(harness.hashed_address(), &mut targets).unwrap();
                let v2_root = calc.compute_root_hash(&nodes).unwrap();

                assert_eq!(recovered_nodes.contains_key(&stale), regenerate_sibling);
                assert_eq!(v2_root, Some(expected_root));
                assert_eq!(recovered_nodes, expected_nodes);
            }
        }
    }

    #[test]
    fn extension_cleanup_skips_live_subtrees() {
        for child in [[0x5, 0xa], [0xf, 0xf]] {
            let child = Nibbles::from_nibbles(child);
            let mut live = child;
            live.push(2);
            let mut stale = child;
            stale.push(7);
            let mut descendant = stale;
            descendant.push(1);
            let mut nodes = BTreeMap::from([
                (live, branch_node(0b11, 0, 0)),
                (stale, branch_node(0b11, 0, 0)),
                (descendant, branch_node(0b11, 0, 0)),
            ]);
            if let Some(next_child) = child.next_without_prefix() {
                nodes.insert(next_child, branch_node(0b11, 0, 0));
            }
            for index in 0..128 {
                let mut path = live;
                path.push(index / 16);
                path.push(index % 16);
                nodes.insert(path, branch_node(0b11, 0, 0));
            }
            let factory = MockTrieCursorFactory::new(nodes, B256Map::default());
            let changes = PrefixSetMut::from([stale]).freeze();
            let mut walker =
                TrieWalker::<_>::state_trie(factory.account_trie_cursor().unwrap(), changes)
                    .unwrap()
                    .with_walk_all_changed_branch_children(true)
                    .with_deletions_retained(true);
            walker.cursor.seek_exact(live).unwrap();
            factory.visited_account_keys().clear();
            walker.remove_orphaned_descendants(child, live, TrieMask::new(u16::MAX)).unwrap();

            assert_eq!(walker.take_removed_keys(), HashSet::from_iter([stale, descendant]));
            assert_eq!(walker.cursor.current().unwrap(), Some(live));
            let next_count = factory
                .visited_account_keys()
                .iter()
                .filter(|visit| matches!(visit.visit_type, KeyVisitType::Next))
                .count();
            assert_eq!(next_count, 2, "cleanup must not traverse live descendants");
        }
    }

    proptest! {
        #[test]
        fn extension_cleanup_respects_bounds_and_recovery_mode(
            prefix in prop::collection::vec(0u8..16, 1..16),
            live_nibble in 0u8..15,
            enabled in any::<bool>(),
            retained in any::<bool>(),
            changed in any::<bool>(),
        ) {
            let child = Nibbles::from_nibbles(&prefix);
            let mut live = child;
            live.push(live_nibble);
            let mut stale = child;
            stale.push(live_nibble + 1);
            let mut live_descendant = live;
            live_descendant.push(0);
            let mut nodes = BTreeMap::from([
                (live, branch_node(0b11, 0, 0)),
                (live_descendant, branch_node(0b11, 0, 0)),
                (stale, branch_node(0b11, 0, 0)),
            ]);
            if let Some(next_child) = child.next_without_prefix() {
                nodes.insert(next_child, branch_node(0b11, 0, 0));
            }
            let factory = MockTrieCursorFactory::new(nodes, B256Map::default());
            let changes = if changed {
                PrefixSetMut::from([stale]).freeze()
            } else {
                PrefixSet::default()
            };
            let mut walker =
                TrieWalker::<_>::state_trie(factory.account_trie_cursor().unwrap(), changes)
                    .unwrap()
                    .with_walk_all_changed_branch_children(enabled)
                    .with_deletions_retained(retained);
            walker.cursor.seek_exact(live).unwrap();
            walker.remove_orphaned_descendants(child, live, TrieMask::new(u16::MAX)).unwrap();
            let expected = if enabled && retained && changed {
                HashSet::from_iter([stale])
            } else {
                HashSet::default()
            };
            prop_assert_eq!(walker.take_removed_keys(), expected);
            prop_assert_eq!(walker.cursor.current().unwrap(), Some(live));
        }
    }
}
