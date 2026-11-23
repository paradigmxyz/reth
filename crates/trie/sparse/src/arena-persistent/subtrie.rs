//! Operations on the lower tries that can be dispatched to separate workers.

use super::{
    branch_data, branch_data_mut, branch_idx,
    trie::{prune_candidate, release_subtree},
    ArenaRequiredProof, ArenaSparseNode, ArenaSparseNodeState, ArenaTrieBuffers, BranchChild,
    BranchChildIdx, NodeArena, NodeIndex, PersistentArenaParallelSparseTrie, RemoveLeafResult,
    SeekResult, TRACE_TARGET,
};
use crate::{arena::take_extension_child, LeafUpdate, SparseTrieUpdates, TrieNodeEpoch};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::B256;
use alloy_trie::TrieMask;
use reth_trie_common::{Nibbles, ProofTrieNode, ProofV2TargetParent, RlpNode};
use tracing::{instrument, trace};

#[cfg(reth_sparse_trie_coverage)]
use super::sparse_guidance::observe_proof_target;

/// A subtrie within the arena-based parallel sparse trie.
///
/// Each subtrie owns its own arena, allowing parallel mutations across subtries.
#[derive(Debug, Clone)]
pub(super) struct ArenaSparseSubtrie {
    /// The arena allocating nodes within this subtrie.
    pub(super) arena: NodeArena,
    /// The root node of this subtrie.
    pub(super) root: NodeIndex,
    /// The absolute path of this subtrie's root in the full trie.
    pub(super) path: Nibbles,
    /// Reusable buffers for traversal, RLP encoding, and update actions.
    pub(super) buffers: ArenaTrieBuffers,
    /// Reusable buffer for collecting required proofs during leaf updates.
    /// Each entry is `(index, proof)` where `index` is the position of the target in the
    /// `sorted_updates` slice passed to [`Self::update_leaves`].
    pub(super) required_proofs: Vec<(usize, ArenaRequiredProof)>,
    /// Total number of revealed leaves in this subtrie.
    pub(super) num_leaves: u64,
    /// Number of dirty (modified since last hash) leaves in this subtrie.
    pub(super) num_dirty_leaves: u64,
}

impl ArenaSparseSubtrie {
    /// Creates a new subtrie with a pre-allocated root slot containing
    /// [`ArenaSparseNode::EmptyRoot`]. The caller must overwrite `subtrie.arena[subtrie.root]`
    /// before use.
    pub(super) fn new(record_updates: bool) -> Box<Self> {
        let mut arena = NodeArena::new();
        let root =
            arena.insert(ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Revealed });
        let buffers = ArenaTrieBuffers {
            updates: record_updates.then(SparseTrieUpdates::default),
            ..Default::default()
        };
        Box::new(Self {
            arena,
            root,
            path: Nibbles::default(),
            buffers,
            required_proofs: Vec::new(),
            num_leaves: 0,
            num_dirty_leaves: 0,
        })
    }

    /// Asserts that `num_leaves` and `num_dirty_leaves` match the actual counts in the arena.
    #[cfg(all(debug_assertions, not(feature = "fuzz")))]
    pub(super) fn debug_assert_counters(&self) {
        let (actual_leaves, actual_dirty) =
            PersistentArenaParallelSparseTrie::count_leaves_and_dirty(&self.arena, self.root);
        debug_assert_eq!(
            self.num_leaves, actual_leaves,
            "subtrie {:?} num_leaves mismatch: stored {} vs actual {}",
            self.path, self.num_leaves, actual_leaves,
        );
        debug_assert_eq!(
            self.num_dirty_leaves, actual_dirty,
            "subtrie {:?} num_dirty_leaves mismatch: stored {} vs actual {}",
            self.path, self.num_dirty_leaves, actual_dirty,
        );
    }

    /// Blinds descendants older than the cutoff and returns the number of revealed slots removed.
    /// The upper trie has checked the epoch cutoff, that the trie is not dirty, and that this
    /// root is kept.
    pub(super) fn prune(&mut self, prune_before: TrieNodeEpoch) -> usize {
        self.arena.reclaim_snapshot_overrides();
        if !self.arena.can_compact() {
            return self.plan_prune(prune_before).map_or(0, |plan| self.apply_prune_plan(plan));
        }

        if branch_idx(&self.arena, self.root).is_none() {
            // A retained leaf can still have vacant slots from edits made while shared.
            if self.arena.has_vacant_slots() {
                self.arena.compact(&mut self.root);
            }
            return 0;
        }

        // Fuse pruning with compaction on the exclusive path instead of building a plan
        // and then walking the retained nodes again.
        let old_count = self.arena.len();
        // Reserve an upper bound on the retained nodes and hand the excess back after the copy,
        // so that copying never reallocates but discarded nodes still release their capacity.
        let mut new_arena = NodeArena::with_capacity(old_count);
        let mut marks = new_arena.adopt_blinded(&mut self.arena);
        let mut new_num_leaves = 0u64;

        // The subtrie root is retained by the owning upper trie.
        let new_root = move_node(&mut new_arena, &mut self.arena, self.root);
        let mut stack = Vec::new();
        if let Some(frame) =
            prepare_retained_node(&new_arena, new_root, self.path, &mut new_num_leaves)
        {
            stack.push(frame);
        }

        while let Some(frame) = stack.last_mut() {
            let Some((child_pos, nibble, child)) = frame.next_child(&new_arena) else {
                stack.pop();
                continue;
            };

            let Some(old_child_idx) = child.revealed_index() else {
                // A blinded child needs no rewrite, its slot survived the adoption.
                marks.mark(child);
                continue;
            };

            let parent_new_idx = frame.new_idx;
            let mut child_path = frame.branch_logical_path;
            child_path.push(nibble);

            if let Some(rlp_node) = prune_candidate(&self.arena, old_child_idx, prune_before) {
                let node = &self.arena[old_child_idx];
                let rlp_node = rlp_node.clone();
                trace!(
                    target: TRACE_TARGET,
                    path = ?child_path,
                    variant = %AsRef::<str>::as_ref(node),
                    cached_rlp_node = ?rlp_node,
                    "pruning node",
                );
                let new_child = new_arena.insert_blinded(rlp_node);
                marks.mark(new_child);
                branch_data_mut(&mut new_arena, parent_new_idx).children[child_pos] = new_child;
                let node = self.arena.drain_node(old_child_idx);
                if let Some(child) = node.extension_child() {
                    self.arena.drain_node(child);
                }
            } else {
                let new_child_idx = move_node(&mut new_arena, &mut self.arena, old_child_idx);
                if let Some(frame) = prepare_retained_node(
                    &new_arena,
                    new_child_idx,
                    child_path,
                    &mut new_num_leaves,
                ) {
                    stack.push(frame);
                }
                branch_data_mut(&mut new_arena, parent_new_idx).children[child_pos] =
                    BranchChild::revealed(new_child_idx);
            }
        }

        new_arena.sweep_blinded(marks);
        new_arena.shrink_nodes_to_fit();
        let pruned = old_count - new_arena.len();
        self.num_leaves = new_num_leaves;
        self.num_dirty_leaves = 0;
        self.arena = new_arena;
        self.root = new_root;

        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_counters();
        return pruned;

        struct CopyFrame {
            new_idx: NodeIndex,
            branch_logical_path: Nibbles,
            state_mask: TrieMask,
            remaining_child_mask: TrieMask,
        }

        impl CopyFrame {
            fn next_child(&mut self, new_arena: &NodeArena) -> Option<(usize, u8, BranchChild)> {
                let b = branch_data(new_arena, self.new_idx);

                let nibble = self.remaining_child_mask.first_set_bit_index()?;
                self.remaining_child_mask.unset_bit(nibble);
                let child_idx = BranchChildIdx::new(self.state_mask, nibble)
                    .expect("remaining_child_mask must be a subset of state_mask");

                Some((child_idx.get(), nibble, b.children[child_idx]))
            }
        }

        /// Prepares a retained node for copying, returning a stack frame when the node has children
        /// to walk.
        fn prepare_retained_node(
            new_arena: &NodeArena,
            new_idx: NodeIndex,
            node_path: Nibbles,
            new_num_leaves: &mut u64,
        ) -> Option<CopyFrame> {
            if branch_idx(new_arena, new_idx).is_none() {
                if matches!(&new_arena[new_idx], ArenaSparseNode::Leaf { .. }) {
                    *new_num_leaves += 1;
                }
                return None;
            }
            let b = branch_data(new_arena, new_idx);

            let mut branch_logical_path = node_path;
            if let Some(key) = new_arena[new_idx].extension_key() {
                branch_logical_path.extend(key);
            }

            Some(CopyFrame {
                new_idx,
                branch_logical_path,
                state_mask: b.state_mask,
                remaining_child_mask: b.state_mask,
            })
        }
    }

    /// Finds the stale descendant roots to blind, without mutating this subtrie or allocating
    /// when no child qualifies. The plan is valid only until the subtrie changes.
    pub(super) fn plan_prune(&self, prune_before: TrieNodeEpoch) -> Option<SubtriePrunePlan> {
        let root_branch = branch_idx(&self.arena, self.root)?;
        let mut cuts = Vec::new();
        let retained_leaves = collect_prune_cuts(&self.arena, root_branch, prune_before, &mut cuts);
        (!cuts.is_empty()).then_some(SubtriePrunePlan { cuts, retained_leaves })
    }

    /// Blinds the planned descendants without copying retained nodes into a new arena unless
    /// compaction is possible. Returns the number of revealed slots removed.
    pub(super) fn apply_prune_plan(&mut self, plan: SubtriePrunePlan) -> usize {
        let old_count = self.arena.len();
        let compact = self.arena.can_compact();
        for cut in plan.cuts {
            trace!(
                target: TRACE_TARGET,
                subtrie = ?self.path,
                child = ?cut.root,
                variant = %AsRef::<str>::as_ref(&self.arena[cut.root]),
                cached_rlp_node = ?cut.rlp_node,
                "pruning node",
            );
            release_subtree(&mut self.arena, cut.root);
            let blinded = self.arena.insert_blinded(cut.rlp_node);
            self.arena[cut.parent].branch_mut().children[cut.position] = blinded;
        }
        // A shared upper slot may contain an exclusive lower arena, e.g. after replacing it.
        // In that case still compact; the plan has already resolved every epoch decision.
        if compact {
            self.arena.compact(&mut self.root);
        }
        self.num_leaves = plan.retained_leaves;
        self.num_dirty_leaves = 0;
        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_counters();
        old_count - self.arena.len()
    }

    /// Applies leaf updates within this subtrie. Uses the same walk-down-with-cursor pattern as
    /// [`Self::reveal_nodes`], but checks accessibility for [`LeafUpdate::Touched`] entries.
    ///
    /// `sorted_updates` must be sorted lexicographically by their nibbles path (index 1).
    ///
    /// Any required proofs are appended to `self.required_proofs` and should be drained by the
    /// caller after this method returns.
    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(
            subtrie = ?self.path,
            num_updates = sorted_updates.len(),
        ),
    )]
    pub(super) fn update_leaves(&mut self, sorted_updates: &[(B256, Nibbles, LeafUpdate)]) {
        if sorted_updates.is_empty() {
            return;
        }
        trace!(target: TRACE_TARGET, "Subtrie update_leaves");

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot { .. }),
            "subtrie root must not be EmptyRoot at start of update_leaves"
        );

        self.buffers.cursor.reset(&self.arena, self.root, self.path);

        for (idx, &(key, ref full_path, ref update)) in sorted_updates.iter().enumerate() {
            let find_result = self.buffers.cursor.seek(&mut self.arena, full_path);

            // If the path hits a blinded node, request a proof regardless of update type.
            if matches!(find_result, SeekResult::Blinded) {
                let logical_len = self.buffers.cursor.head_logical_branch_path_len(&self.arena);
                #[cfg(reth_sparse_trie_coverage)]
                observe_proof_target(
                    &self.arena,
                    self.buffers.cursor.head().map(|entry| (entry.index, logical_len)),
                    full_path,
                    &[],
                );
                self.required_proofs.push((
                    idx,
                    ArenaRequiredProof { key, parent: ProofV2TargetParent::new(logical_len) },
                ));
                continue;
            }

            match update {
                LeafUpdate::Changed(value) if !value.is_empty() => {
                    // Upsert: insert or update a leaf with the given value.
                    let (_result, deltas) = PersistentArenaParallelSparseTrie::upsert_leaf(
                        &mut self.arena,
                        &mut self.buffers.cursor,
                        &mut self.root,
                        full_path,
                        value,
                        find_result,
                    );
                    self.num_leaves = (self.num_leaves as i64 + deltas.num_leaves_delta) as u64;
                    self.num_dirty_leaves =
                        (self.num_dirty_leaves as i64 + deltas.num_dirty_leaves_delta) as u64;
                }
                LeafUpdate::Changed(_) => {
                    let (result, deltas) = PersistentArenaParallelSparseTrie::remove_leaf(
                        &mut self.arena,
                        &mut self.buffers.cursor,
                        &mut self.root,
                        key,
                        full_path,
                        find_result,
                        &mut self.buffers.updates,
                    );
                    self.num_leaves = (self.num_leaves as i64 + deltas.num_leaves_delta) as u64;
                    self.num_dirty_leaves =
                        (self.num_dirty_leaves as i64 + deltas.num_dirty_leaves_delta) as u64;

                    // The removed leaf is already revealed, so only the blinded sibling needs a
                    // proof. The caller retries the update once that sibling is revealed.
                    if let RemoveLeafResult::NeedsProof { proof_key, parent, .. } = result {
                        self.required_proofs
                            .push((idx, ArenaRequiredProof { key: proof_key, parent }));
                    }
                }
                LeafUpdate::Touched => {}
            }
        }

        // Drain remaining cursor entries, propagating dirty state.
        self.buffers.cursor.drain(&mut self.arena);

        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_counters();
    }

    /// Reveals nodes inside this subtrie. Uses [`super::cursor::ArenaCursor::seek`] to locate the
    /// ancestor node, then replaces blinded children with the proof nodes.
    pub(super) fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNode]) {
        if nodes.is_empty() {
            return;
        }
        trace!(target: TRACE_TARGET, path = ?self.path, num_nodes = nodes.len(), "Subtrie reveal_nodes");

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot { .. }),
            "subtrie root must not be EmptyRoot in reveal_nodes"
        );

        self.buffers.cursor.reset(&self.arena, self.root, self.path);

        let mut node_idx = 0;
        while node_idx < nodes.len() {
            let extension_child = take_extension_child(nodes, node_idx);
            let next_idx = node_idx + 1 + usize::from(extension_child.is_some());
            let node = &mut nodes[node_idx];
            let find_result = self.buffers.cursor.seek(&mut self.arena, &node.path);
            if PersistentArenaParallelSparseTrie::reveal_node(
                &mut self.arena,
                &self.buffers.cursor,
                node,
                extension_child,
                find_result,
            )
            .is_some_and(|child_idx| matches!(self.arena[child_idx], ArenaSparseNode::Leaf { .. }))
            {
                self.num_leaves += 1;
            }
            node_idx = next_idx;
        }

        // Drain remaining cursor entries, propagating dirty state.
        self.buffers.cursor.drain(&mut self.arena);

        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_counters();
    }

    /// Computes and caches `RlpNode` for all dirty nodes via iterative post-order DFS.
    /// After this call every node reachable from `self.root` will be in `Cached` state.
    ///
    /// Trie updates are written directly to `self.buffers.updates` (if `Some`).
    pub(super) fn update_cached_rlp(&mut self, new_epoch: TrieNodeEpoch) {
        PersistentArenaParallelSparseTrie::update_cached_rlp(
            &mut self.arena,
            self.root,
            self.path,
            &mut self.buffers,
            new_epoch,
        );
        self.num_dirty_leaves = 0;
        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
        self.debug_assert_counters();
    }

    /// Copies lower storage independently even if it is prepared for snapshots.
    pub(super) fn clone_independent(&self) -> Self {
        Self {
            arena: self.arena.deep_clone(Clone::clone),
            root: self.root,
            path: self.path,
            buffers: self.buffers.clone(),
            required_proofs: self.required_proofs.clone(),
            num_leaves: self.num_leaves,
            num_dirty_leaves: self.num_dirty_leaves,
        }
    }
}

/// Moves a node into another arena, keeping an extension with its branch child.
fn move_node(dst: &mut NodeArena, src: &mut NodeArena, idx: NodeIndex) -> NodeIndex {
    let node = src.drain_node(idx);
    let new_idx = dst.insert(node);
    if let Some(child_idx) = dst[new_idx].extension_child() {
        let child = src.drain_node(child_idx);
        let new_child_idx = dst.insert(child);
        let ArenaSparseNode::Extension { child, .. } = &mut dst[new_idx] else { unreachable!() };
        *child = new_child_idx;
    }
    new_idx
}

/// Disjoint stale subtrees under a retained lower root, and its leaf count after blinding them.
pub(super) struct SubtriePrunePlan {
    cuts: Vec<PruneCut>,
    retained_leaves: u64,
}

fn collect_prune_cuts(
    arena: &NodeArena,
    parent: NodeIndex,
    prune_before: TrieNodeEpoch,
    cuts: &mut Vec<PruneCut>,
) -> u64 {
    let mut leaves = 0;
    for (position, child) in arena[parent].branch_ref().children.iter().enumerate() {
        let Some(root) = child.revealed_index() else { continue };
        if let Some(rlp_node) = prune_candidate(arena, root, prune_before) {
            cuts.push(PruneCut { parent, position, root, rlp_node: rlp_node.clone() });
        } else if let Some(branch) = branch_idx(arena, root) {
            leaves += collect_prune_cuts(arena, branch, prune_before, cuts);
        } else if matches!(&arena[root], ArenaSparseNode::Leaf { .. }) {
            leaves += 1;
        }
    }
    leaves
}

struct PruneCut {
    parent: NodeIndex,
    position: usize,
    root: NodeIndex,
    rlp_node: RlpNode,
}
