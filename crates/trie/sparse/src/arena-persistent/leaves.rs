//! Leaf edits and the branch changes they require.

use super::{
    branch_data, branch_data_mut, branch_idx, ArenaCursor, ArenaRequiredProof, ArenaSparseNode,
    ArenaSparseNodeBranch, ArenaSparseNodeState, BranchChild, NodeArena, NodeIndex,
    PersistentArenaParallelSparseTrie, RemoveLeafResult, SeekResult, SubtrieCounterDeltas,
    UpsertLeafResult, TRACE_TARGET,
};
use crate::{LeafUpdate, SparseTrieUpdates};
use alloy_primitives::B256;
use alloy_trie::TrieMask;
use core::mem;
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofV2TargetParent};
use smallvec::SmallVec;
use tracing::{instrument, trace};

#[cfg(all(feature = "fuzz", reth_sparse_trie_coverage))]
use super::sparse_guidance::observe_proof_target;

impl PersistentArenaParallelSparseTrie {
    /// Splits the leaf or extension at the cursor head and inserts a new leaf.
    ///
    /// `new_leaf_path` is relative to the cursor head's path. The two paths must diverge within
    /// the old node's key. The split replaces the cursor head with the new branch or extension.
    ///
    /// Returns `true` only if the old node was a leaf and this split marked it dirty.
    fn split_and_insert_leaf(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut NodeIndex,
        new_leaf_path: Nibbles,
        value: &[u8],
    ) -> bool {
        let old_child_idx = cursor.head().expect("cursor must have head").index;
        let old_child_short_key =
            arena[old_child_idx].short_key().expect("split requires a leaf or extension");
        let diverge_len = new_leaf_path.common_prefix_length(old_child_short_key);

        trace!(
            target: TRACE_TARGET,
            path = ?cursor.head_path(),
            ?new_leaf_path,
            ?old_child_short_key,
            diverge_len,
            "Splitting node and inserting new leaf",
        );

        let old_child_nibble = old_child_short_key.get_unchecked(diverge_len);
        let old_child_suffix = old_child_short_key.slice(diverge_len + 1..);

        // Truncate the old child's key/extension key and mark it dirty.
        // Track whether the existing node was not already dirty (a leaf that becomes newly dirty).
        let old_child = mem::replace(&mut arena[old_child_idx], ArenaSparseNode::TakenSubtrie);
        let newly_dirtied_existing = match old_child {
            ArenaSparseNode::Leaf { value, state, .. } => {
                let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                arena[old_child_idx] = ArenaSparseNode::Leaf {
                    key: old_child_suffix,
                    value,
                    state: ArenaSparseNodeState::Dirty,
                };
                was_clean
            }
            ArenaSparseNode::Extension { child, .. } => {
                if old_child_suffix.is_empty() {
                    let branch = arena.remove(child).expect("extension child exists");
                    arena[old_child_idx] = branch;
                    *arena[old_child_idx].state_mut() = ArenaSparseNodeState::Dirty;
                } else {
                    arena[old_child_idx] =
                        ArenaSparseNode::Extension { key: old_child_suffix, child };
                    *arena[child].state_mut() = ArenaSparseNodeState::Dirty;
                }
                // Branches don't contribute to num_dirty_leaves.
                false
            }
            _ => unreachable!("split_and_insert_leaf called on non-Leaf/Extension node"),
        };

        let short_key = new_leaf_path.slice(..diverge_len);
        let new_leaf_nibble = new_leaf_path.get_unchecked(diverge_len);
        debug_assert_ne!(old_child_nibble, new_leaf_nibble);

        let new_leaf_idx = arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Dirty,
            key: new_leaf_path.slice(diverge_len + 1..),
            value: value.to_vec(),
        });

        let (first_nibble, first_child, second_nibble, second_child) =
            if old_child_nibble < new_leaf_nibble {
                (old_child_nibble, old_child_idx, new_leaf_nibble, new_leaf_idx)
            } else {
                (new_leaf_nibble, new_leaf_idx, old_child_nibble, old_child_idx)
            };

        let state_mask = TrieMask::from_nibble(first_nibble) | TrieMask::from_nibble(second_nibble);
        let mut children = SmallVec::new();
        children.push(BranchChild::revealed(first_child));
        children.push(BranchChild::revealed(second_child));

        let branch_child_idx = arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
            state: ArenaSparseNodeState::Dirty,
            children,
            state_mask,
            branch_masks: BranchNodeMasks::default(),
        }));
        let new_branch_idx = if short_key.is_empty() {
            branch_child_idx
        } else {
            arena.insert(ArenaSparseNode::Extension { key: short_key, child: branch_child_idx })
        };

        cursor.replace_head_index(arena, root, new_branch_idx);
        newly_dirtied_existing
    }

    /// Performs a leaf upsert using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// Updates an existing leaf in place, replaces an empty root, adds a missing branch
    /// child, or splits a leaf or extension whose key diverges from the new leaf's path.
    ///
    /// The caller must handle [`SeekResult::Blinded`] and
    /// [`SeekResult::RevealedSubtrie`] before calling this function.
    /// The cursor must be non-empty when called.
    ///
    /// Returns an [`UpsertLeafResult`] and [`SubtrieCounterDeltas`] so the caller can maintain
    /// aggregate counters and decide whether to wrap the result as a subtrie.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, fields(full_path = ?full_path))]
    pub(super) fn upsert_leaf(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut NodeIndex,
        full_path: &Nibbles,
        value: &[u8],
        find_result: SeekResult,
    ) -> (UpsertLeafResult, SubtrieCounterDeltas) {
        trace!(target: TRACE_TARGET, ?find_result, "Upserting leaf");
        let head = cursor.head().expect("cursor is non-empty");

        match find_result {
            SeekResult::Blinded => {
                unreachable!("Blinded case must be handled by caller")
            }
            SeekResult::EmptyRoot => {
                let head_idx = head.index;
                arena[head_idx] = ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Dirty,
                    key: full_path.slice(cursor.head_path_len()..),
                    value: value.to_vec(),
                };
                (
                    UpsertLeafResult::NewLeaf,
                    SubtrieCounterDeltas { num_leaves_delta: 1, num_dirty_leaves_delta: 1 },
                )
            }
            SeekResult::RevealedLeaf => {
                // RevealedLeaf guarantees the leaf's full path matches the target exactly.
                let head_idx = head.index;
                let was_clean =
                    if let ArenaSparseNode::Leaf { value: v, state, .. } = &mut arena[head_idx] {
                        v.clear();
                        v.extend_from_slice(value);
                        let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                        *state = ArenaSparseNodeState::Dirty;
                        was_clean
                    } else {
                        unreachable!("RevealedLeaf but cursor head is not a leaf")
                    };
                (
                    UpsertLeafResult::Updated,
                    SubtrieCounterDeltas {
                        num_leaves_delta: 0,
                        num_dirty_leaves_delta: was_clean as i64,
                    },
                )
            }
            SeekResult::Diverged => {
                let full_path_from_head = full_path.slice(cursor.head_path_len()..);

                let split_dirtied_existing =
                    Self::split_and_insert_leaf(arena, cursor, root, full_path_from_head, value);

                let result = if cursor.depth() >= 1 {
                    UpsertLeafResult::NewChild
                } else {
                    UpsertLeafResult::NewLeaf
                };
                (
                    result,
                    SubtrieCounterDeltas {
                        num_leaves_delta: 1,
                        num_dirty_leaves_delta: 1 + split_dirtied_existing as i64,
                    },
                )
            }
            SeekResult::NoChild { child_nibble } => {
                let head_idx = head.index;

                let leaf_key = full_path.slice(cursor.head_logical_branch_path_len(arena) + 1..);
                let new_leaf = arena.insert(ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Dirty,
                    key: leaf_key,
                    value: value.to_vec(),
                });

                let branch = branch_data_mut(arena, head_idx);
                branch.set_child(child_nibble, BranchChild::revealed(new_leaf));

                // Re-seek to position the cursor on the newly inserted leaf.
                cursor.seek(arena, full_path);

                (
                    UpsertLeafResult::NewChild,
                    SubtrieCounterDeltas { num_leaves_delta: 1, num_dirty_leaves_delta: 1 },
                )
            }
            SeekResult::RevealedSubtrie => {
                unreachable!("RevealedSubtrie must be handled by caller")
            }
        }
    }

    /// Removes a leaf node from the trie using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// Only the `RevealedLeaf` case performs a removal — the leaf must exist and its full path
    /// must match `full_path`. All other cases (`Diverged`, `NoChild`) are no-ops since the leaf
    /// doesn't exist at that path.
    ///
    /// When removing a leaf from a branch, if the branch is left with only one remaining child,
    /// the branch is collapsed: the remaining child absorbs the extension key and child's nibble
    /// as a prefix to its own key or extension key, and replaces the branch in the parent.
    /// If the remaining child is blinded, the collapse cannot proceed and a
    /// [`RemoveLeafResult::NeedsProof`] is returned so the caller can request a proof.
    ///
    /// The caller must handle [`SeekResult::Blinded`] and
    /// [`SeekResult::RevealedSubtrie`] before calling this function.
    pub(super) fn remove_leaf(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut NodeIndex,
        key: B256,
        full_path: &Nibbles,
        find_result: SeekResult,
        updates: &mut Option<SparseTrieUpdates>,
    ) -> (RemoveLeafResult, SubtrieCounterDeltas) {
        match find_result {
            SeekResult::Blinded | SeekResult::RevealedSubtrie => {
                unreachable!("Blinded/RevealedSubtrie must be handled by caller")
            }
            SeekResult::EmptyRoot | SeekResult::Diverged | SeekResult::NoChild { .. } => {
                (RemoveLeafResult::NotFound, SubtrieCounterDeltas::default())
            }
            SeekResult::RevealedLeaf => {
                // RevealedLeaf guarantees the leaf's full path matches the target exactly.
                let head_idx = cursor.head().expect("cursor is non-empty").index;
                let head_path = cursor.head_path();

                trace!(
                    target: TRACE_TARGET,
                    path = ?head_path,
                    ?full_path,
                    "Removing leaf",
                );

                // Before mutating, check if removing this leaf would leave the parent
                // branch with a single blinded sibling (requiring a proof to collapse).
                if let Some(proof) = Self::collapse_sibling_proof(arena, cursor) {
                    return (
                        RemoveLeafResult::NeedsProof {
                            key,
                            proof_key: proof.key,
                            parent: proof.parent,
                        },
                        SubtrieCounterDeltas::default(),
                    );
                }

                // Check if the removed leaf was dirty before removing it.
                let removed_was_dirty =
                    matches!(arena[head_idx].state_ref(), Some(ArenaSparseNodeState::Dirty));

                if cursor.depth() == 0 {
                    // The leaf is the root — replace with EmptyRoot and reset the cursor
                    // so subsequent iterations can call seek normally.
                    arena.discard(head_idx);
                    *root = arena
                        .insert(ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Dirty });
                    cursor.reset(arena, *root, head_path);
                    return (
                        RemoveLeafResult::Removed,
                        SubtrieCounterDeltas {
                            num_leaves_delta: -1,
                            num_dirty_leaves_delta: -(removed_was_dirty as i64),
                        },
                    );
                }

                // Pop the leaf entry, propagating dirty state to the parent.
                cursor.pop(arena);

                // The parent must be a branch. Remove the leaf from it.
                let parent_entry = cursor.head().expect("cursor is non-empty");
                let parent_idx = parent_entry.index;
                let child_nibble = head_path.last().expect("non-root leaf");

                // Remove the leaf from the arena and from the parent's children.
                arena.discard(head_idx);
                let parent_branch = branch_data_mut(arena, parent_idx);
                parent_branch.remove_child(child_nibble);

                // If the branch now has only one child, collapse it. The blinded sibling
                // case was already handled above before any mutations.
                let collapse_dirtied_leaf = if parent_branch.state_mask.count_bits() == 1 {
                    Self::collapse_branch(arena, cursor, root, updates)
                } else {
                    false
                };
                (
                    RemoveLeafResult::Removed,
                    SubtrieCounterDeltas {
                        num_leaves_delta: -1,
                        num_dirty_leaves_delta: (collapse_dirtied_leaf as i64) -
                            (removed_was_dirty as i64),
                    },
                )
            }
        }
    }

    /// Checks whether a subtrie receiving only removals would cause its parent branch to collapse
    /// onto a single blinded sibling. If so, returns the proof needed to reveal that blinded
    /// sibling so the caller can request it and skip the subtrie's updates.
    ///
    /// Returns `Some(proof)` for the blinded sibling when the edge-case applies, `None` otherwise.
    pub(super) fn check_subtrie_collapse_needs_proof(
        arena: &NodeArena,
        cursor: &ArenaCursor,
        subtrie_updates: &[(B256, Nibbles, LeafUpdate)],
    ) -> Option<ArenaRequiredProof> {
        let num_removals = subtrie_updates
            .iter()
            .filter(|(_, _, u)| matches!(u, LeafUpdate::Changed(v) if v.is_empty()))
            .count() as u64;

        // Touched is a no-op that doesn't alter trie structure, so it must be
        // excluded when deciding whether "all updates are removals". This mirrors
        // the `all_removals` / `might_empty_subtrie` filter in `update_leaves`.
        // Without this, a batch of removals + Touched entries
        // would fail the `num_removals != num_changed` check, skip the proof
        // request for the blinded sibling, and later panic in
        // `maybe_collapse_or_remove_branch` when the subtrie empties inline.
        let num_changed = subtrie_updates.iter().filter(|(_, _, u)| u.is_changed()).count() as u64;

        if num_removals == 0 || num_removals != num_changed {
            return None;
        }

        // The subtrie is the cursor head; its parent is the cursor's parent.
        let subtrie_entry = cursor.head()?;
        let subtrie_num_leaves = match &arena[subtrie_entry.index] {
            ArenaSparseNode::Subtrie(s) => s.num_leaves,
            _ => return None,
        };
        if num_removals < subtrie_num_leaves {
            return None;
        }

        Self::collapse_sibling_proof(arena, cursor)
    }

    /// Returns the proof needed if removing the cursor head would collapse its parent onto a
    /// blinded sibling. A cursor on the root has no sibling.
    fn collapse_sibling_proof(
        arena: &NodeArena,
        cursor: &ArenaCursor,
    ) -> Option<ArenaRequiredProof> {
        let parent_entry = cursor.parent()?;
        let child_nibble =
            cursor.head_last_nibble().expect("non-root child path must have a nibble");
        let parent_branch = branch_data(arena, parent_entry.index);
        if parent_branch.state_mask.count_bits() != 2 {
            return None;
        }

        if !parent_branch.sibling_child(child_nibble).is_blinded() {
            return None;
        }

        let sibling_nibble = parent_branch
            .state_mask
            .iter()
            .find(|&n| n != child_nibble)
            .expect("branch has two children");
        let mut sibling_path = cursor.parent_logical_branch_path(arena);
        let parent_depth = sibling_path.len();
        sibling_path.push_unchecked(sibling_nibble);
        trace!(target: TRACE_TARGET, ?sibling_path, "Collapse onto blinded sibling requires a proof");

        #[cfg(all(feature = "fuzz", reth_sparse_trie_coverage))]
        observe_proof_target(arena, Some((parent_entry.index, parent_depth)), &sibling_path, &[]);
        Some(ArenaRequiredProof {
            key: Self::nibbles_to_padded_b256(&sibling_path),
            parent: ProofV2TargetParent::new(parent_depth),
        })
    }

    /// Collapses a branch node that has exactly one remaining revealed child. The branch's
    /// extension key, the remaining child's nibble, and the child's own key or extension key are
    /// concatenated to form the child's new key or extension key. The child then replaces the
    /// branch in the grandparent (or becomes the new root).
    ///
    /// The caller must verify that the remaining child is not blinded before calling this function.
    ///
    /// The branch being collapsed must be the current cursor head. The cursor head will be
    /// replaced with the remaining child which has taken its place.
    /// Returns `true` if the collapse dirtied a surviving leaf that was not already dirty.
    pub(super) fn collapse_branch(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut NodeIndex,
        updates: &mut Option<SparseTrieUpdates>,
    ) -> bool {
        let branch_node_idx = cursor.head().expect("cursor is non-empty").index;
        let branch_child_idx = branch_idx(arena, branch_node_idx).expect("cursor head is a branch");
        let branch_path = cursor.head_path();
        let branch = arena[branch_child_idx].branch_ref();
        let remaining_nibble =
            branch.state_mask.iter().next().expect("branch has at least one child");
        let branch_short_key = arena[branch_node_idx].extension_key().copied().unwrap_or_default();

        debug_assert_eq!(
            branch.state_mask.count_bits(),
            1,
            "collapse_branch requires exactly 1 child"
        );
        debug_assert!(
            !branch.children[0].is_blinded(),
            "collapse_branch called with a blinded remaining child"
        );

        trace!(
            target: TRACE_TARGET,
            path = ?branch_path,
            short_key = ?branch_short_key,
            branch_masks = ?branch.branch_masks,
            ?remaining_nibble,
            "Collapsing single-child branch",
        );

        // Record the collapsed branch's logical path for trie update tracking if it
        // was previously persisted in the DB trie.
        if let Some(trie_updates) = updates.as_mut() &&
            !branch.branch_masks.is_empty()
        {
            let logical_path = cursor.head_logical_branch_path(arena);
            if !logical_path.is_empty() {
                trie_updates.updated_nodes.remove(&logical_path);
                trie_updates.removed_nodes.insert(logical_path);
            }
        }

        // Build the prefix: extension key + remaining child's nibble.
        let mut prefix = branch_short_key;
        prefix.push_unchecked(remaining_nibble);

        let child_idx = branch.children[0].revealed_index().expect("remaining child is revealed");

        // Prepend the prefix to the child's key or extension key and mark dirty.
        // Track whether a leaf was newly dirtied by this collapse.
        let child = mem::replace(&mut arena[child_idx], ArenaSparseNode::TakenSubtrie);
        let (replacement_idx, newly_dirtied_leaf) = match child {
            ArenaSparseNode::Leaf { key, value, state } => {
                let mut new_key = prefix;
                new_key.extend(&key);
                let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                arena[child_idx] = ArenaSparseNode::Leaf {
                    key: new_key,
                    state: ArenaSparseNodeState::Dirty,
                    value,
                };
                (child_idx, was_clean)
            }
            ArenaSparseNode::Extension { key, child } => {
                let mut new_key = prefix;
                new_key.extend(&key);
                arena[child_idx] = ArenaSparseNode::Extension { key: new_key, child };
                *arena[child].state_mut() = ArenaSparseNodeState::Dirty;
                (child_idx, false)
            }
            ArenaSparseNode::Branch(branch) => {
                let new_branch_idx = arena.insert(ArenaSparseNode::Branch(branch));
                *arena[new_branch_idx].state_mut() = ArenaSparseNodeState::Dirty;
                arena[child_idx] =
                    ArenaSparseNode::Extension { key: prefix, child: new_branch_idx };
                (child_idx, false)
            }
            ArenaSparseNode::Subtrie(mut subtrie) => {
                subtrie.path = branch_path;
                let root = subtrie.root;
                let root_node =
                    mem::replace(&mut subtrie.arena[root], ArenaSparseNode::TakenSubtrie);
                match root_node {
                    ArenaSparseNode::Branch(branch) => {
                        let child = subtrie.arena.insert(ArenaSparseNode::Branch(branch));
                        subtrie.arena[root] = ArenaSparseNode::Extension { key: prefix, child };
                        *subtrie.arena[child].state_mut() = ArenaSparseNodeState::Dirty;
                    }
                    ArenaSparseNode::Extension { key, child } => {
                        let mut new_key = prefix;
                        new_key.extend(&key);
                        subtrie.arena[root] = ArenaSparseNode::Extension { key: new_key, child };
                        *subtrie.arena[child].state_mut() = ArenaSparseNodeState::Dirty;
                    }
                    ArenaSparseNode::Leaf { key, value, state } => {
                        let mut new_key = prefix;
                        new_key.extend(&key);
                        let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                        subtrie.arena[root] = ArenaSparseNode::Leaf {
                            key: new_key,
                            value,
                            state: ArenaSparseNodeState::Dirty,
                        };
                        if was_clean {
                            subtrie.num_dirty_leaves += 1;
                        }
                    }
                    _ => {
                        unreachable!("subtrie root must be a Branch or Leaf during collapse_branch")
                    }
                }
                arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);
                (child_idx, false)
            }
            _ => unreachable!("remaining child must be Leaf, Branch, Extension, or Subtrie"),
        };

        // Replace the branch with the remaining child in the grandparent (or root).
        cursor.replace_head_index(arena, root, replacement_idx);

        // Free the collapsed branch.
        arena.discard(branch_node_idx);
        if branch_node_idx != branch_child_idx {
            arena.discard(branch_child_idx);
        }
        newly_dirtied_leaf
    }
}
