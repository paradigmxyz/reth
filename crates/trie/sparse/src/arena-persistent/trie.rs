//! Sparse trie trait implementation and persistent pruning.

use super::{
    branch_data_mut, branch_idx, node_state_ref, ArenaCursor, ArenaSparseNode,
    ArenaSparseNodeBranch, ArenaSparseNodeState, ArenaSparseSubtrie, BranchChild, BranchChildIdx,
    NextResult, NodeArena, NodeIndex, PersistentArenaParallelSparseTrie, RemoveLeafResult,
    SeekResult, UpsertLeafResult, TRACE_TARGET,
};
use crate::{
    LeafLookup, LeafLookupError, LeafUpdate, SparseTrie, SparseTrieUpdates, TrieNodeEpoch,
};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_primitives::{map::B256Map, B256};
use core::{cmp::Reverse, mem};
use rayon::iter::{IntoParallelIterator, IntoParallelRefMutIterator, ParallelIterator};
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{
    BranchNodeMasks, Nibbles, ProofTrieNodeV2, ProofV2TargetParent, RlpNode, TrieNodeV2,
};
use smallvec::SmallVec;
use tracing::{instrument, trace};

#[cfg(all(feature = "fuzz", reth_sparse_trie_coverage))]
use super::sparse_guidance::observe_proof_target;

impl SparseTrie for PersistentArenaParallelSparseTrie {
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn set_root(
        &mut self,
        root: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<()> {
        debug_assert!(
            matches!(self.upper_arena[self.root], ArenaSparseNode::EmptyRoot { .. }),
            "set_root called on a trie that already has revealed nodes"
        );

        match root {
            TrieNodeV2::EmptyRoot => {
                trace!(target: TRACE_TARGET, "Setting empty root");
                self.upper_arena[self.root] =
                    ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Revealed };
            }
            TrieNodeV2::Leaf(leaf) => {
                trace!(target: TRACE_TARGET, key = ?leaf.key, "Setting leaf root");
                self.upper_arena[self.root] = ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Revealed,
                    key: leaf.key,
                    value: leaf.value,
                };
            }
            TrieNodeV2::Branch(branch) => {
                trace!(target: TRACE_TARGET, state_mask = ?branch.state_mask, num_children = branch.state_mask.count_bits(), "Setting branch root");
                let mut children = SmallVec::with_capacity(branch.state_mask.count_bits() as usize);
                for (stack_ptr, _nibble) in branch.state_mask.iter().enumerate() {
                    let child = self.upper_arena.insert_blinded(branch.stack[stack_ptr].clone());
                    children.push(child);
                }

                let branch_node = ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed,
                    children,
                    state_mask: branch.state_mask,
                    branch_masks: masks.unwrap_or_default(),
                });
                if branch.key.is_empty() {
                    self.upper_arena[self.root] = branch_node;
                } else {
                    let child = self.upper_arena.insert(branch_node);
                    self.upper_arena[self.root] =
                        ArenaSparseNode::Extension { key: branch.key, child };
                }
            }
            TrieNodeV2::Extension(node) => {
                return Err(SparseTrieErrorKind::Reveal {
                    path: Nibbles::new(),
                    node: Box::new(node),
                }
                .into());
            }
        }

        self.set_updates(retain_updates);
        Ok(())
    }

    fn set_updates(&mut self, retain_updates: bool) {
        self.buffers.reset_updates(retain_updates);
        self.upper_arena.for_each_mut_if(
            |node| {
                let ArenaSparseNode::Subtrie(subtrie) = node else { return false };
                match &subtrie.buffers.updates {
                    None => retain_updates,
                    Some(updates) => {
                        !retain_updates ||
                            !updates.updated_nodes.is_empty() ||
                            !updates.removed_nodes.is_empty()
                    }
                }
            },
            |_, node| {
                let ArenaSparseNode::Subtrie(subtrie) = node else { unreachable!() };
                subtrie.buffers.reset_updates(retain_updates);
            },
        );
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, fields(num_nodes = nodes.len()))]
    fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        if matches!(self.upper_arena[self.root], ArenaSparseNode::EmptyRoot { .. }) {
            trace!(target: TRACE_TARGET, "Skipping reveal_nodes on empty root");
            return Ok(());
        }

        // Sort nodes lexicographically by path.
        nodes.sort_unstable_by_key(|n| n.path);

        let threshold = self.parallelism_thresholds.min_revealed_nodes;

        // Take the cursor out to avoid borrow conflicts with `self`.
        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // Skip root node if present (set_root handles the root).
        let mut node_idx = if nodes[0].path.is_empty() { 1 } else { 0 };

        // Walk the upper trie, revealing upper nodes inline and collecting subtrie work.
        // Subtries with enough nodes to reveal are taken for parallel processing; the rest
        // are revealed inline.
        let mut taken: Vec<(NodeIndex, Box<ArenaSparseSubtrie>, Vec<ProofTrieNodeV2>)> = Vec::new();

        while node_idx < nodes.len() {
            let find_result = cursor.seek(&mut self.upper_arena, &nodes[node_idx].path);

            match find_result {
                SeekResult::RevealedLeaf => {
                    trace!(target: TRACE_TARGET, path = ?nodes[node_idx].path, "Skipping reveal: leaf head");
                    node_idx += 1;
                }
                SeekResult::Blinded => {
                    // Save the proof node's path before reveal_node consumes it.
                    let child_path = nodes[node_idx].path;
                    let child_idx = Self::reveal_node(
                        &mut self.upper_arena,
                        &cursor,
                        &mut nodes[node_idx],
                        SeekResult::Blinded,
                    );
                    node_idx += 1;

                    if let Some(child_idx) = child_idx {
                        self.maybe_wrap_in_subtrie(child_idx, &child_path);
                    }
                }
                SeekResult::RevealedSubtrie => {
                    let child_idx = cursor.head().expect("cursor is non-empty").index;
                    let prefix = cursor.head_path();

                    let subtrie_start = node_idx;
                    while node_idx < nodes.len() && nodes[node_idx].path.starts_with(&prefix) {
                        node_idx += 1;
                    }
                    let num_subtrie_nodes = node_idx - subtrie_start;

                    if num_subtrie_nodes >= threshold {
                        // Take subtrie for parallel reveal.
                        trace!(target: TRACE_TARGET, ?prefix, num_subtrie_nodes, "Taking subtrie for parallel reveal");
                        let ArenaSparseNode::Subtrie(subtrie) = mem::replace(
                            &mut self.upper_arena[child_idx],
                            ArenaSparseNode::TakenSubtrie,
                        ) else {
                            unreachable!("RevealedSubtrie must point to a Subtrie node")
                        };
                        let node_vec: Vec<ProofTrieNodeV2> = (subtrie_start..node_idx)
                            .map(|i| mem::replace(&mut nodes[i], ProofTrieNodeV2::empty()))
                            .collect();
                        taken.push((child_idx, subtrie, node_vec));
                    } else {
                        // Reveal inline.
                        trace!(target: TRACE_TARGET, ?prefix, num_subtrie_nodes, "Revealing subtrie inline");
                        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[child_idx]
                        else {
                            unreachable!("RevealedSubtrie must point to a Subtrie node")
                        };
                        subtrie.reveal_nodes(&mut nodes[subtrie_start..node_idx]);
                    }
                }
                _ => {
                    trace!(target: TRACE_TARGET, path = ?nodes[node_idx].path, ?find_result, "Skipping reveal: no blinded child");
                    node_idx += 1;
                }
            }
        }

        // Drain remaining cursor entries from the upper-trie walk.
        cursor.drain(&mut self.upper_arena);
        self.buffers.cursor = cursor;

        if taken.is_empty() {
            return Ok(());
        }

        // Reveal taken subtries, in parallel if more than one.
        if taken.len() == 1 {
            let (_, subtrie, node_vec) = &mut taken[0];
            subtrie.reveal_nodes(node_vec);
        } else {
            let parent_span = tracing::Span::current();
            taken.par_iter_mut().for_each(|(_, subtrie, node_vec)| {
                let _guard = parent_span.enter();
                subtrie.reveal_nodes(node_vec);
            });
        }

        // Restore taken subtries into the upper arena.
        for (idx, subtrie, _) in taken {
            self.restore_taken_subtrie(idx, subtrie);
        }

        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_subtrie_structure();

        Ok(())
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, ret)]
    fn root(&mut self, new_epoch: TrieNodeEpoch) -> B256 {
        self.update_subtrie_hashes(new_epoch);

        let rlp_node = Self::update_cached_rlp(
            &mut self.upper_arena,
            self.root,
            Nibbles::default(),
            &mut self.buffers,
            new_epoch,
        );

        rlp_node.as_hash().expect("root RlpNode must be a hash")
    }

    fn is_root_cached(&self) -> bool {
        matches!(
            node_state_ref(&self.upper_arena, self.root),
            Some(ArenaSparseNodeState::Cached { .. })
        )
    }

    fn root_epoch(&self) -> Option<TrieNodeEpoch> {
        match node_state_ref(&self.upper_arena, self.root)? {
            ArenaSparseNodeState::Revealed => Some(TrieNodeEpoch::UNMODIFIED),
            ArenaSparseNodeState::Cached { epoch, .. } => Some(*epoch),
            ArenaSparseNodeState::Dirty => None,
        }
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn update_subtrie_hashes(&mut self, new_epoch: TrieNodeEpoch) {
        trace!(target: TRACE_TARGET, "Updating subtrie hashes");

        // Only descend if the root is a branch or extension; otherwise there are no subtries.
        if branch_idx(&self.upper_arena, self.root).is_none() {
            return;
        }

        // Count total dirty leaves across all subtries to make one global parallelism decision.
        let mut total_dirty_leaves: u64 = 0;
        let mut taken: Vec<(NodeIndex, Box<ArenaSparseSubtrie>)> = Vec::new();
        // Select before borrowing mutably so clean shared nodes stay shared.
        self.upper_arena.for_each_mut_if(
            |node| matches!(node, ArenaSparseNode::Subtrie(s) if s.num_dirty_leaves > 0),
            |idx, node| {
                let ArenaSparseNode::Subtrie(subtrie) =
                    mem::replace(node, ArenaSparseNode::TakenSubtrie)
                else {
                    unreachable!()
                };
                total_dirty_leaves += subtrie.num_dirty_leaves;
                taken.push((idx, subtrie));
            },
        );

        // Hash taken subtries in parallel if total dirty leaves meet the threshold.
        if !taken.is_empty() {
            if taken.len() == 1 || total_dirty_leaves < self.parallelism_thresholds.min_dirty_leaves
            {
                for (_, subtrie) in &mut taken {
                    subtrie.update_cached_rlp(new_epoch);
                }
            } else {
                let parent_span = tracing::Span::current();
                taken = taken
                    .into_par_iter()
                    .map(|(idx, mut subtrie)| {
                        let _guard = parent_span.enter();
                        subtrie.update_cached_rlp(new_epoch);
                        (idx, subtrie)
                    })
                    .collect();
            }
        }

        // If the root branch is already cached and nothing was taken for parallel
        // hashing, there are no dirty subtries to process.
        if taken.is_empty() && self.is_root_cached() {
            return;
        }

        // Walk the upper trie depth-first, restoring hashed subtries and inline-hashing
        // any remaining dirty subtries. Only descend into dirty branches; clean subtrees
        // cannot contain dirty subtries since dirty state propagates upward.
        taken.sort_unstable_by_key(|(_, b)| Reverse(b.path));

        self.buffers.cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        loop {
            let result = self.buffers.cursor.next(&mut self.upper_arena, |_, child| match child {
                ArenaSparseNode::Branch(_) | ArenaSparseNode::Subtrie(_) => !child.is_cached(),
                ArenaSparseNode::Extension { .. } | ArenaSparseNode::TakenSubtrie => true,
                _ => false,
            });

            match result {
                NextResult::Done => break,
                NextResult::Branch => continue,
                NextResult::NonBranch => {}
            }

            // Head is a subtrie or taken-subtrie — process it.
            let head_idx = self.buffers.cursor.head().expect("cursor is non-empty").index;

            if matches!(&self.upper_arena[head_idx], ArenaSparseNode::TakenSubtrie) {
                let (_, subtrie) = taken.pop().expect("taken subtries must not be exhausted");
                debug_assert_eq!(
                    subtrie.path,
                    self.buffers.cursor.head_path(),
                    "taken subtrie path mismatch",
                );
                self.upper_arena[head_idx] = ArenaSparseNode::Subtrie(subtrie);
            }

            self.update_upper_subtrie(head_idx, new_epoch);
        }
    }

    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>> {
        Self::get_leaf_value_in_arena(&self.upper_arena, self.root, full_path, 0)
    }

    fn find_leaf(
        &self,
        full_path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        Self::find_leaf_in_arena(&self.upper_arena, self.root, full_path, 0, expected_value)
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.buffers
            .updates
            .as_ref()
            .map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        match self.buffers.updates.take() {
            Some(updates) => {
                self.buffers.updates = Some(SparseTrieUpdates::with_capacity(
                    updates.updated_nodes.len(),
                    updates.removed_nodes.len(),
                ));
                updates
            }
            None => SparseTrieUpdates::default(),
        }
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn clear(&mut self) {
        self.upper_arena = NodeArena::new();
        self.root = self
            .upper_arena
            .insert(ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Revealed });
        self.buffers.clear();
    }

    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(prune_before = prune_before.get()),
    )]
    fn prune(&mut self, prune_before: TrieNodeEpoch) -> usize {
        assert!(self.root_epoch().is_some(), "prune cannot run on a dirty trie");

        // A previous prune could only override shared upper slots. Once their last peer
        // drops, reclaim their old values even if this pass has nothing left to prune.
        self.upper_arena.reclaim_snapshot_overrides();

        // No epoch is older than UNMODIFIED, and only branches and extensions have children.
        if prune_before == TrieNodeEpoch::UNMODIFIED ||
            branch_idx(&self.upper_arena, self.root).is_none()
        {
            if self.upper_arena.has_vacant_slots() && self.upper_arena.can_compact() {
                self.upper_arena.compact(&mut self.root);
            }
            return 0;
        }

        let threshold = self.parallelism_thresholds.min_leaves_for_prune;
        let plan_before_write = self.upper_arena.nodes_mutation_copies();

        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // When taking a slot might copy it, plan large subtries in parallel before taking them.
        let mut pending_plans = Vec::new();
        let mut taken = Vec::new();

        let mut pruned = 0;

        loop {
            let result = cursor.next(&mut self.upper_arena, |_, child| {
                matches!(
                    child,
                    ArenaSparseNode::Branch(_) |
                        ArenaSparseNode::Extension { .. } |
                        ArenaSparseNode::Subtrie(_) |
                        ArenaSparseNode::Leaf { .. }
                )
            });

            if matches!(result, NextResult::Done) {
                break
            }

            let head_idx = cursor.head().expect("cursor is non-empty").index;

            match &self.upper_arena[head_idx] {
                ArenaSparseNode::Branch(_) |
                ArenaSparseNode::Extension { .. } |
                ArenaSparseNode::Leaf { .. } => {
                    // Don't prune the root.
                    if cursor.depth() == 0 {
                        continue;
                    }

                    if prune_candidate(&self.upper_arena, head_idx, prune_before).is_none() {
                        continue;
                    }

                    let removed = Self::remove_pruned_node(&mut self.upper_arena, &mut cursor);
                    pruned += 1 + usize::from(removed.extension_child().is_some());
                }
                ArenaSparseNode::Subtrie(subtrie) => {
                    if prune_candidate(&self.upper_arena, head_idx, prune_before).is_some() {
                        let removed = Self::remove_pruned_node(&mut self.upper_arena, &mut cursor);
                        let ArenaSparseNode::Subtrie(s) = &removed else { unreachable!() };
                        pruned += s.arena.len();
                        self.recycle_subtrie(removed);
                        continue;
                    }

                    let parallel = subtrie.num_leaves >= threshold;
                    if plan_before_write && self.upper_arena.node_mutation_copies(head_idx) {
                        if parallel {
                            pending_plans.push(head_idx);
                        } else if let Some(plan) = subtrie.plan_prune(prune_before) {
                            let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx]
                            else {
                                unreachable!()
                            };
                            pruned += subtrie.apply_prune_plan(plan);
                        }
                    } else if parallel {
                        taken.push((
                            head_idx,
                            take_subtrie_for_pruning(&mut self.upper_arena, head_idx),
                            Some(()),
                        ));
                    } else {
                        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx]
                        else {
                            unreachable!()
                        };
                        pruned += subtrie.prune(prune_before);
                    }
                }
                _ => unreachable!("prune walk must visit a subtrie, leaf, branch, or extension"),
            }
        }

        self.buffers.cursor = cursor;

        pruned += self.prune_taken_subtries(taken, |subtrie, ()| subtrie.prune(prune_before));
        if !pending_plans.is_empty() {
            // A surviving subtrie cannot be removed by pruning an ancestor: the ancestor's
            // cached epoch includes the subtrie's epoch.
            let plan = |idx| {
                let ArenaSparseNode::Subtrie(subtrie) = &self.upper_arena[idx] else {
                    unreachable!()
                };
                subtrie.plan_prune(prune_before).map(|plan| (idx, plan))
            };
            let planned: Vec<_> = if pending_plans.len() == 1 {
                plan(pending_plans[0]).into_iter().collect()
            } else {
                let parent_span = tracing::Span::current();
                pending_plans
                    .into_par_iter()
                    .filter_map(|idx| {
                        let _guard = parent_span.enter();
                        plan(idx)
                    })
                    .collect()
            };
            let taken = planned
                .into_iter()
                .map(|(idx, plan)| {
                    (idx, take_subtrie_for_pruning(&mut self.upper_arena, idx), Some(plan))
                })
                .collect();
            pruned += self.prune_taken_subtries(taken, ArenaSparseSubtrie::apply_prune_plan);
        }

        // Vacancies from a shared prune can outlive the last snapshot without new evictions.
        if (pruned > 0 || self.upper_arena.has_vacant_slots()) && self.upper_arena.can_compact() {
            self.upper_arena.compact(&mut self.root);
        }

        pruned
    }

    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(num_updates = updates.len()),
    )]
    fn update_leaves(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        mut proof_required_fn: impl FnMut(B256, ProofV2TargetParent),
    ) -> SparseTrieResult<()> {
        if updates.is_empty() {
            return Ok(());
        }

        // Drain and sort updates lexicographically by nibbles path.
        let mut sorted: Vec<_> =
            updates.drain().map(|(key, update)| (key, Nibbles::unpack(key), update)).collect();
        sorted.sort_unstable_by_key(|entry| entry.1);

        let threshold = self.parallelism_thresholds.min_updates;
        let parallelize_distributed_updates = sorted.len() >= threshold.saturating_mul(4);

        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // Subtries taken for parallel processing: (arena_index, subtrie, update_range).
        let mut taken: Vec<(NodeIndex, Box<ArenaSparseSubtrie>, core::ops::Range<usize>)> =
            Vec::new();

        let mut update_idx = 0;
        while update_idx < sorted.len() {
            let (key, ref full_path, ref update) = sorted[update_idx];

            let find_result = cursor.seek(&mut self.upper_arena, full_path);

            match find_result {
                // Blinded — request a proof regardless of update type.
                SeekResult::Blinded => {
                    let logical_len = cursor.head_logical_branch_path_len(&self.upper_arena);
                    let parent = ProofV2TargetParent::new(logical_len);
                    trace!(target: TRACE_TARGET, ?key, ?parent, "Update hit blinded node, requesting proof");
                    #[cfg(all(feature = "fuzz", reth_sparse_trie_coverage))]
                    observe_proof_target(
                        &self.upper_arena,
                        cursor.head().map(|entry| (entry.index, logical_len)),
                        full_path,
                        &taken,
                    );
                    proof_required_fn(key, parent);
                    updates.insert(key, update.clone());
                }
                // Subtrie — forward all consecutive updates under this subtrie's prefix.
                SeekResult::RevealedSubtrie => {
                    let child_idx = cursor.head().expect("cursor is non-empty").index;
                    let subtrie_root_path = cursor.head_path();

                    let subtrie_start = update_idx;
                    while update_idx < sorted.len() &&
                        sorted[update_idx].1.starts_with(&subtrie_root_path)
                    {
                        update_idx += 1;
                    }

                    let subtrie_updates = &sorted[subtrie_start..update_idx];

                    // Edge-case: if all updates are removals that could empty the
                    // subtrie and collapse the parent onto a blinded sibling, request
                    // a proof for the sibling and skip the subtrie's updates.
                    if let Some(proof) = Self::check_subtrie_collapse_needs_proof(
                        &self.upper_arena,
                        &cursor,
                        subtrie_updates,
                    ) {
                        trace!(target: TRACE_TARGET, proof_key = ?proof.key, proof_parent = ?proof.parent, "Subtrie collapse would need blinded sibling, requesting proof");
                        proof_required_fn(proof.key, proof.parent);
                        for &(key, _, ref update) in subtrie_updates {
                            updates.insert(key, update.clone());
                        }
                        continue;
                    }

                    let num_subtrie_updates = update_idx - subtrie_start;

                    // If all updates are removals and could empty the subtrie,
                    // force inline processing so the upper-arena collapse logic
                    // can detect blinded siblings and request proofs.
                    let all_removals = subtrie_updates
                        .iter()
                        // Filter out Touched, as they don't affect the structure of the trie. So an
                        // update set with 2 removals and one Touched could still result in an empty
                        // sub trie.
                        .filter(|(_, _, u)| u.is_changed())
                        .all(|(_, _, u)| matches!(u, LeafUpdate::Changed(v) if v.is_empty()));
                    let subtrie_num_leaves = match &self.upper_arena[child_idx] {
                        ArenaSparseNode::Subtrie(s) => s.num_leaves,
                        _ => 0,
                    };
                    let might_empty_subtrie =
                        all_removals && num_subtrie_updates as u64 >= subtrie_num_leaves;

                    if (num_subtrie_updates >= threshold || parallelize_distributed_updates) &&
                        !might_empty_subtrie
                    {
                        // Take subtrie for parallel update.
                        trace!(target: TRACE_TARGET, ?subtrie_root_path, num_subtrie_updates, "Taking subtrie for parallel update");
                        let ArenaSparseNode::Subtrie(subtrie) = mem::replace(
                            &mut self.upper_arena[child_idx],
                            ArenaSparseNode::TakenSubtrie,
                        ) else {
                            unreachable!()
                        };
                        taken.push((child_idx, subtrie, subtrie_start..update_idx));
                    } else {
                        // Update inline.
                        trace!(target: TRACE_TARGET, ?subtrie_root_path, num_subtrie_updates, "Updating subtrie inline");
                        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[child_idx]
                        else {
                            unreachable!()
                        };

                        subtrie.update_leaves(subtrie_updates);

                        for (target_idx, proof) in subtrie.required_proofs.drain(..) {
                            proof_required_fn(proof.key, proof.parent);
                            let (key, _, ref update) = subtrie_updates[target_idx];
                            updates.insert(key, update.clone());
                        }

                        // Check if the subtrie's root became empty after updates.
                        self.maybe_unwrap_subtrie(&mut cursor);
                    }

                    // Don't increment update_idx — already advanced past subtrie updates.
                    continue;
                }
                // EmptyRoot, leaf, diverged branch, or empty child slot — upsert directly.
                find_result @ (SeekResult::EmptyRoot |
                SeekResult::RevealedLeaf |
                SeekResult::Diverged |
                SeekResult::NoChild { .. }) => match update {
                    LeafUpdate::Changed(v) if !v.is_empty() => {
                        let (result, _deltas) = Self::upsert_leaf(
                            &mut self.upper_arena,
                            &mut cursor,
                            &mut self.root,
                            full_path,
                            v,
                            find_result,
                        );
                        match result {
                            UpsertLeafResult::NewChild => {
                                if Self::should_be_subtrie(cursor.head_path_len()) {
                                    // The new child itself sits at the subtrie
                                    // boundary — wrap it directly.
                                    let head_idx =
                                        cursor.head().expect("cursor is non-empty").index;
                                    self.maybe_wrap_in_subtrie(head_idx, &cursor.head_path());
                                } else {
                                    // The new child is above the boundary (e.g. a
                                    // split at depth 1 creates children at depth 2).
                                    // Wrap any of its children that land there.
                                    self.maybe_wrap_branch_children(&cursor);
                                }
                            }
                            UpsertLeafResult::NewLeaf => {
                                // A root-level split may create children at the
                                // subtrie boundary depth. Wrap them.
                                self.maybe_wrap_branch_children(&cursor);
                            }
                            UpsertLeafResult::Updated => {}
                        }
                    }
                    LeafUpdate::Changed(_) => {
                        let (result, _deltas) = Self::remove_leaf(
                            &mut self.upper_arena,
                            &mut cursor,
                            &mut self.root,
                            key,
                            full_path,
                            find_result,
                            &mut self.buffers.updates,
                        );
                        match result {
                            RemoveLeafResult::NeedsProof { key, proof_key, parent } => {
                                proof_required_fn(proof_key, parent);
                                let update =
                                    mem::replace(&mut sorted[update_idx].2, LeafUpdate::Touched);
                                updates.insert(key, update);
                            }
                            RemoveLeafResult::Removed => {
                                // remove_leaf may have called collapse_branch, which
                                // can leave structural invariants violated:
                                // 1. A branch with 0-1 children that needs further collapse or
                                //    removal.
                                // 2. A Subtrie at a depth shallower than UPPER_TRIE_MAX_DEPTH that
                                //    needs unwrapping.
                                // 3. A non-Subtrie node at UPPER_TRIE_MAX_DEPTH that needs
                                //    wrapping.
                                self.maybe_collapse_or_remove_branch(&mut cursor);
                                let head_idx = cursor
                                    .head()
                                    .expect("cursor always has root after collapse")
                                    .index;
                                self.maybe_wrap_in_subtrie(head_idx, &cursor.head_path());
                            }
                            RemoveLeafResult::NotFound => {}
                        }
                    }
                    LeafUpdate::Touched => {}
                },
            }

            update_idx += 1;
        }

        // Drain remaining cursor entries from the upper-trie walk.
        cursor.drain(&mut self.upper_arena);
        self.buffers.cursor = cursor;

        if taken.is_empty() {
            #[cfg(all(debug_assertions, not(feature = "fuzz")))]
            self.debug_assert_subtrie_structure();

            return Ok(());
        }

        // Apply updates to taken subtries, in parallel if more than one.
        if taken.len() == 1 {
            let (_, ref mut subtrie, ref range) = taken[0];
            subtrie.update_leaves(&sorted[range.clone()]);
        } else {
            let parent_span = tracing::Span::current();
            taken.par_iter_mut().for_each(|(_, subtrie, range)| {
                let _guard = parent_span.enter();
                subtrie.update_leaves(&sorted[range.clone()]);
            });
        }

        // Collect subtrie paths before consuming `taken`, then restore subtries and
        // process required proofs.
        let taken_paths: Vec<Nibbles> = taken.iter().map(|(_, s, _)| s.path).collect();
        for (child_idx, mut subtrie, range) in taken {
            let subtrie_updates = &sorted[range];
            for (target_idx, proof) in subtrie.required_proofs.drain(..) {
                proof_required_fn(proof.key, proof.parent);
                let (key, _, ref update) = subtrie_updates[target_idx];
                updates.insert(key, update.clone());
            }

            // Restore the subtrie into the upper arena.
            self.restore_taken_subtrie(child_idx, subtrie);
        }

        // Navigate to each taken subtrie via seek to propagate dirty state
        // through intermediate branches. Taken subtries are guaranteed not to
        // become EmptyRoot (the would-empty-subtrie check above forces those
        // inline), so we only need to handle sibling collapses that may have
        // occurred during inline processing while this subtrie was taken.
        {
            let mut cursor = mem::take(&mut self.buffers.cursor);
            cursor.reset(&self.upper_arena, self.root, Nibbles::default());

            for path in &taken_paths {
                let find_result = cursor.seek(&mut self.upper_arena, path);
                match find_result {
                    SeekResult::RevealedSubtrie => {
                        debug_assert!(
                            {
                                let head_idx = cursor.head().expect("cursor is non-empty").index;
                                !matches!(
                                    &self.upper_arena[head_idx],
                                    ArenaSparseNode::Subtrie(s) if matches!(s.arena[s.root], ArenaSparseNode::EmptyRoot { .. })
                                )
                            },
                            "taken subtrie became EmptyRoot — should have been forced inline"
                        );

                        cursor.pop(&mut self.upper_arena);

                        // The parent branch (now at cursor top) may have had a sibling
                        // removed during inline processing while this subtrie was taken.
                        // Handle any necessary collapse or removal.
                        self.maybe_collapse_or_remove_branch(&mut cursor);
                    }
                    _ => {
                        // Subtrie was already unwrapped by a prior collapse; dirty state
                        // was propagated during that collapse. Nothing to do.
                    }
                }
            }

            cursor.drain(&mut self.upper_arena);
            self.buffers.cursor = cursor;
        }

        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_subtrie_structure();

        Ok(())
    }

    fn snapshot(&mut self) -> Self {
        Self {
            upper_arena: self.upper_arena.snapshot(),
            root: self.root,
            buffers: self.buffers.clone(),
            parallelism_thresholds: self.parallelism_thresholds,
        }
    }
}

impl PersistentArenaParallelSparseTrie {
    /// Removes the hashed, non-root node at the cursor head and blinds its parent's child slot.
    ///
    /// Reclaims same-arena descendants if the caller cannot compact the arena afterward.
    pub(super) fn remove_pruned_node(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
    ) -> ArenaSparseNode {
        // Entry paths are derived from the cursor's path, so read the head path before popping.
        let path = cursor.head_path();
        let idx = cursor.head().expect("cursor is non-empty").index;
        let rlp_node = cached_prune_state(arena, idx).0.clone();
        cursor.pop(arena);
        let node = arena.remove(idx).expect("node must exist to be pruned");
        if !arena.can_compact() {
            release_descendants(arena, &node);
        } else if let Some(child) = node.extension_child() {
            arena.discard(child);
        }
        trace!(
            target: TRACE_TARGET,
            ?path,
            variant = %AsRef::<str>::as_ref(&node),
            cached_rlp_node = ?rlp_node,
            "pruning node",
        );

        let parent_idx = cursor.head().expect("pruned child has parent").index;
        let child_nibble = path.last().expect("non-root child");
        let blinded = arena.insert_blinded(rlp_node);
        let parent_branch = branch_data_mut(arena, parent_idx);
        let child_idx = BranchChildIdx::new(parent_branch.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");
        parent_branch.children[child_idx] = blinded;

        node
    }

    /// Applies prepared work to taken subtries and restores their upper slots.
    fn prune_taken_subtries<P: Send>(
        &mut self,
        mut taken: Vec<(NodeIndex, Box<ArenaSparseSubtrie>, Option<P>)>,
        prune: impl Fn(&mut ArenaSparseSubtrie, P) -> usize + Sync,
    ) -> usize {
        let pruned = match taken.as_mut_slice() {
            [] => return 0,
            [(_, subtrie, plan)] => prune(subtrie, plan.take().expect("prune work is present")),
            _ => {
                let parent_span = tracing::Span::current();
                taken
                    .par_iter_mut()
                    .map(|(_, subtrie, plan)| {
                        let _guard = parent_span.enter();
                        let _span = tracing::trace_span!(
                            target: TRACE_TARGET,
                            "subtrie_prune",
                            subtrie = ?subtrie.path,
                        )
                        .entered();
                        prune(subtrie, plan.take().expect("prune work is present"))
                    })
                    .sum()
            }
        };
        for (idx, subtrie, _) in taken {
            self.restore_taken_subtrie(idx, subtrie);
        }
        pruned
    }
}

fn take_subtrie_for_pruning(arena: &mut NodeArena, idx: NodeIndex) -> Box<ArenaSparseSubtrie> {
    let ArenaSparseNode::Subtrie(subtrie) =
        mem::replace(&mut arena[idx], ArenaSparseNode::TakenSubtrie)
    else {
        unreachable!()
    };
    subtrie
}

/// Releases a node's descendants in this arena when a later compaction will not reclaim them.
fn release_descendants(arena: &mut NodeArena, detached: &ArenaSparseNode) {
    match detached {
        ArenaSparseNode::Extension { child, .. } => release_subtree(arena, *child),
        ArenaSparseNode::Branch(branch) => release_branch_children(arena, &branch.children),
        _ => {}
    }
}

/// Releases the revealed nodes and blinded child slots of a detached subtree.
pub(super) fn release_subtree(arena: &mut NodeArena, idx: NodeIndex) {
    match &arena[idx] {
        ArenaSparseNode::Extension { child, .. } => release_subtree(arena, *child),
        ArenaSparseNode::Branch(branch) => {
            let children = branch.children.clone();
            release_branch_children(arena, &children);
        }
        _ => {}
    }
    arena.discard(idx);
}

fn release_branch_children(arena: &mut NodeArena, children: &[BranchChild]) {
    for &child in children {
        if let Some(idx) = child.revealed_index() {
            release_subtree(arena, idx);
        } else {
            arena.discard_blinded(child);
        }
    }
}

/// Returns the cached RLP if this node predates the cutoff. The trie must already be hashed.
#[inline]
pub(super) fn prune_candidate(
    arena: &NodeArena,
    idx: NodeIndex,
    prune_before: TrieNodeEpoch,
) -> Option<&RlpNode> {
    let (rlp_node, epoch) = cached_prune_state(arena, idx);
    epoch.should_prune(prune_before).then_some(rlp_node)
}

/// Pruning requires a non-dirty root. Unlike the root, revealed descendants carry cached
/// encodings, whether they were hashed locally or revealed by a proof.
#[inline]
fn cached_prune_state(arena: &NodeArena, idx: NodeIndex) -> (&RlpNode, TrieNodeEpoch) {
    let Some(ArenaSparseNodeState::Cached { rlp_node, epoch }) = node_state_ref(arena, idx) else {
        panic!("prune must run after hashing");
    };
    (rlp_node, *epoch)
}
