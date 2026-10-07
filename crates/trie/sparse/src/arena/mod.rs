//! Parallel sparse trie built from independently allocated arenas.

mod branch_child_idx;
mod cursor;
mod hashing;
mod mutation;
mod node_arena;
mod nodes;
mod subtrie;

use branch_child_idx::{BranchChildIdx, BranchChildIter};
use cursor::{ArenaCursor, NextResult, SeekResult};
use node_arena::{BranchChild, Index, NodeArena};
use nodes::{ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeState};

use crate::{
    LeafLookup, LeafLookupError, LeafUpdate, SparseTrie, SparseTrieUpdates, TrieNodeEpoch,
};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_primitives::{keccak256, map::B256Map, B256};
use alloy_trie::TrieMask;
use core::{cmp::Reverse, mem};
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{
    BranchNodeMasks, BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, ProofTrieNodeV2,
    ProofV2TargetParent, RlpNode, TrieNodeV2, EMPTY_ROOT_HASH,
};
use smallvec::SmallVec;
use tracing::{instrument, trace};

const TRACE_TARGET: &str = "trie::arena";

/// The maximum path length (in nibbles) for nodes that live in the upper trie. Nodes at this
/// depth or deeper belong to lower subtries.
const UPPER_TRIE_MAX_DEPTH: usize = 2;

/// Compacts an arena by BFS-copying all reachable nodes into a fresh arena, dropping
/// unreachable (pruned) slots. Parents are stored before children for cache-friendly top-down
/// traversal.
fn compact_arena(arena: &mut NodeArena, root: &mut Index) {
    let mut new_arena = NodeArena::with_capacity(arena.len());
    let mut marks = new_arena.adopt_blinded(arena);
    let root_node = arena.drain_node(*root);
    let new_root = new_arena.insert(root_node);

    // Nodes append consecutively without removals, so the destination vector is also the BFS
    // queue. Each visited node's children still reference the source arena until rewritten.
    let mut next = 0;
    while next < new_arena.len() {
        let new_idx = Index::new(next);
        next += 1;
        let old_children: SmallVec<[(usize, Index); 16]> = match &new_arena[new_idx] {
            ArenaSparseNode::Branch(b) => b
                .children
                .iter()
                .enumerate()
                .filter_map(|(pos, child)| match child.revealed_index() {
                    Some(old_child_idx) => Some((pos, old_child_idx)),
                    // A blinded child needs no rewrite, its slot survived the adoption.
                    None => {
                        marks.mark(*child);
                        None
                    }
                })
                .collect(),
            _ => continue,
        };

        for (child_pos, old_child_idx) in old_children {
            let child_node = arena.drain_node(old_child_idx);
            let new_child_idx = new_arena.insert(child_node);
            debug_assert_eq!(
                new_child_idx.get() + 1,
                new_arena.len(),
                "compaction must append, the destination doubles as the BFS queue",
            );
            let ArenaSparseNode::Branch(b) = &mut new_arena[new_idx] else { unreachable!() };
            b.children[child_pos] = BranchChild::revealed(new_child_idx);
        }
    }

    debug_assert!(
        arena.iter().next().is_none(),
        "compact_arena: {} orphaned nodes remaining after BFS drain",
        arena.iter().count(),
    );

    new_arena.sweep_blinded(marks);
    *arena = new_arena;
    *root = new_root;
}

/// Reusable traversal state and optional accumulators shared by
/// [`ArenaSparseSubtrie`] and [`ArenaParallelSparseTrie`].
#[derive(Debug, Default, Clone)]
struct ArenaTrieBuffers {
    /// Reusable cursor for trie traversals.
    cursor: ArenaCursor,
    /// Trie updates built up directly during hashing and structural changes. `Some` when
    /// tracking updates, `None` otherwise. Initialized alongside `updates` in `set_updates`.
    updates: Option<SparseTrieUpdates>,
    /// Reusable buffer for RLP encoding.
    rlp_buf: Vec<u8>,
    /// Reusable buffer for child `RlpNode`s during hashing.
    rlp_node_buf: Vec<RlpNode>,
}

impl ArenaTrieBuffers {
    fn clear(&mut self) {
        if let Some(updates) = self.updates.as_mut() {
            updates.clear();
        }
        self.rlp_buf.clear();
        self.rlp_node_buf.clear();
    }
}

/// A subtrie within the arena-based parallel sparse trie.
///
/// Each subtrie owns its own arena, allowing parallel mutations across subtries.
#[derive(Debug, Clone)]
struct ArenaSparseSubtrie {
    /// The arena allocating nodes within this subtrie.
    arena: NodeArena,
    /// The root node of this subtrie.
    root: Index,
    /// The absolute path of this subtrie's root in the full trie.
    path: Nibbles,
    /// Reusable buffers for traversal, RLP encoding, and update actions.
    buffers: ArenaTrieBuffers,
    /// Reusable buffer for collecting required proofs during leaf updates.
    /// Each entry is `(index, proof)` where `index` is the position of the target in the
    /// `sorted_updates` slice passed to [`Self::update_leaves`].
    required_proofs: Vec<(usize, ArenaRequiredProof)>,
    /// Total number of revealed leaves in this subtrie.
    num_leaves: u64,
    /// Number of dirty (modified since last hash) leaves in this subtrie.
    num_dirty_leaves: u64,
}

/// Tracks the net change in leaf counters caused by a trie mutation (upsert or removal).
/// Returned alongside [`UpsertLeafResult`] / [`RemoveLeafResult`] so the caller can maintain
/// aggregate counters on [`ArenaSparseSubtrie`] without scanning the arena.
#[derive(Debug, Default)]
struct SubtrieCounterDeltas {
    num_leaves_delta: i64,
    num_dirty_leaves_delta: i64,
}

/// Result of `upsert_leaf` indicating whether a new child was created that the caller
/// may need to wrap as a subtrie (in the upper trie).
#[derive(Debug)]
enum UpsertLeafResult {
    /// A leaf was updated in place (no structural change).
    Updated,
    /// A new leaf was created (e.g. EmptyRoot→Leaf, or root-level split).
    NewLeaf,
    /// A new child (branch or leaf) was created or inserted. The child is the cursor head
    /// and its parent is the cursor's parent.
    NewChild,
}

/// Result of `remove_leaf` indicating whether a proof is needed to complete a branch
/// collapse.
#[derive(Debug)]
enum RemoveLeafResult {
    /// No proof needed — the removal (and any collapse) completed fully.
    Removed,
    /// No leaf was found at the given path (no-op).
    NotFound,
    /// The branch collapse requires revealing a blinded sibling. The caller must request a
    /// proof for the given key below the revealed logical parent branch.
    NeedsProof { key: B256, proof_key: B256, parent: ProofV2TargetParent },
}

/// A proof request generated during leaf updates when a blinded node is encountered.
#[derive(Debug, Clone)]
struct ArenaRequiredProof {
    /// The key requiring a proof.
    key: B256,
    /// The revealed logical parent branch.
    parent: ProofV2TargetParent,
}

/// An arena-based parallel sparse trie.
///
/// Configuration for controlling when parallelism is enabled in [`ArenaParallelSparseTrie`]
/// operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaParallelismThresholds {
    /// Minimum number of dirty leaves in a subtrie before it is eligible for parallel hash
    /// computation. Subtries with fewer dirty leaves than this are hashed serially during
    /// [`ArenaParallelSparseTrie::update_subtrie_hashes`].
    pub min_dirty_leaves: u64,
    /// Minimum number of nodes to reveal in a subtrie before it is eligible for parallel
    /// reveal. Subtries with fewer nodes to reveal than this are revealed inline during the
    /// upper trie walk.
    pub min_revealed_nodes: usize,
    /// Minimum number of leaf updates targeting a subtrie before it is eligible for parallel
    /// update. Subtries with fewer updates than this are updated inline during the upper trie
    /// walk.
    pub min_updates: usize,
    /// Minimum number of revealed leaves in a subtrie before it is eligible for parallel
    /// pruning. Subtries with fewer leaves than this are pruned inline during the upper trie
    /// walk.
    pub min_leaves_for_prune: u64,
}

impl Default for ArenaParallelismThresholds {
    fn default() -> Self {
        Self {
            min_dirty_leaves: 64,
            min_revealed_nodes: 16,
            min_updates: 128,
            min_leaves_for_prune: 128,
        }
    }
}

/// An arena-based sparse trie whose subtries can be mutated in parallel.
///
/// ## Structure
///
/// Uses arena allocation (`NodeArena`) for node storage with direct index-based child
/// pointers, avoiding the per-node hashing overhead of a `HashMap`-based trie. The trie is split
/// into two tiers:
///
/// - **Upper trie** (`upper_arena`): Contains nodes whose path is shorter than
///   `UPPER_TRIE_MAX_DEPTH` nibbles. These are the root and its immediate children.
/// - **Lower subtries** (`ArenaSparseSubtrie`): Each child of an upper-trie branch at the depth
///   boundary becomes the root of its own subtrie, stored as an `ArenaSparseNode::Subtrie` child in
///   the upper arena. Each subtrie owns its own arena, enabling lock-free parallel mutation.
///
/// Node placement is determined by path length (not counting a branch's short key):
///
/// - Paths with **< `UPPER_TRIE_MAX_DEPTH`** nibbles live in `upper_arena`.
/// - Paths with **≥ `UPPER_TRIE_MAX_DEPTH`** nibbles live in a subtrie.
///
/// ## Node Revealing
///
/// Nodes are lazily revealed from proof data via [`SparseTrie::reveal_nodes`]. Each node is
/// placed into the upper arena or delegated to its subtrie based on path depth. Unrevealed
/// children are stored as blinded `BranchChild`s referencing their RLP encoding.
/// When multiple subtries have pending reveals, they are processed in parallel using rayon
/// (controlled by [`ArenaParallelismThresholds::min_revealed_nodes`]).
///
/// ## Leaf Operations
///
/// Leaf updates and removals are applied via [`SparseTrie::update_leaves`]. The method walks
/// the upper trie to route each update to the correct subtrie, then processes subtries in
/// parallel when the update count exceeds [`ArenaParallelismThresholds::min_updates`].
///
/// After updates, structural changes (branch collapse, subtrie unwrapping) are handled by
/// propagating dirty state back up through the upper trie.
///
/// ## Root Hash Calculation
///
/// Root hash computation follows a bottom-up approach:
///
/// 1. **[`SparseTrie::update_subtrie_hashes`]**: Takes dirty subtries from the upper arena and
///    hashes them in parallel (when dirty leaf count meets
///    [`ArenaParallelismThresholds::min_dirty_leaves`]), then walks the upper trie to restore
///    hashed subtries and inline-hash any remaining dirty nodes.
/// 2. **[`SparseTrie::root`]**: Calls `update_subtrie_hashes`, then RLP-encodes the full upper trie
///    depth-first to produce the root hash.
///
/// Each node tracks its state via `ArenaSparseNodeState` (`Revealed`, `Cached`, or `Dirty`)
/// so only modified subtrees are recomputed.
///
/// ## Pruning
///
/// [`SparseTrie::prune`] replaces nodes older than its epoch cutoff with
/// blinded `BranchChild` entries using their cached RLP, then compacts the
/// arenas. Subtries are pruned in parallel when their leaf count exceeds
/// [`ArenaParallelismThresholds::min_leaves_for_prune`].
#[derive(Debug, Clone)]
pub struct ArenaParallelSparseTrie {
    /// The arena allocating nodes in the upper trie.
    upper_arena: NodeArena,
    /// The root node of the upper trie.
    root: Index,
    /// Reusable buffers for traversal, RLP encoding, and update actions.
    buffers: ArenaTrieBuffers,
    /// Thresholds controlling when parallelism is enabled for different operations.
    parallelism_thresholds: ArenaParallelismThresholds,
}

impl ArenaParallelSparseTrie {
    /// Sets the thresholds that control when parallelism is used during operations.
    pub const fn with_parallelism_thresholds(
        mut self,
        thresholds: ArenaParallelismThresholds,
    ) -> Self {
        self.parallelism_thresholds = thresholds;
        self
    }

    /// Returns `true` if a node at the given path length should be placed in a subtrie rather
    /// than the upper arena.
    const fn should_be_subtrie(path_len: usize) -> bool {
        path_len == UPPER_TRIE_MAX_DEPTH
    }

    /// If the child at the cursor head should be a subtrie based on its depth, wraps it
    /// in [`ArenaSparseNode::Subtrie`].
    ///
    /// The child must be the cursor head and its parent the cursor's parent.
    fn maybe_wrap_in_subtrie(&mut self, child_idx: Index, child_path: &Nibbles) {
        if !Self::should_be_subtrie(child_path.len()) {
            return;
        }

        // Only branch and leaf nodes can become subtrie roots.
        if !matches!(
            self.upper_arena[child_idx],
            ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. }
        ) {
            return;
        }

        trace!(target: TRACE_TARGET, ?child_path, "Wrapping child into subtrie");
        let mut subtrie = ArenaSparseSubtrie::new(self.buffers.updates.is_some());
        subtrie.path = *child_path;
        let mut root_node =
            mem::replace(&mut self.upper_arena[child_idx], ArenaSparseNode::TakenSubtrie);

        // Migrate any children from the upper arena into the subtrie arena.
        Self::migrate_children(&mut subtrie.arena, &mut self.upper_arena, &mut root_node);

        subtrie.arena[subtrie.root] = root_node;
        let (leaves, dirty) = Self::count_leaves_and_dirty(&subtrie.arena, subtrie.root);
        subtrie.num_leaves = leaves;
        subtrie.num_dirty_leaves = dirty;
        #[cfg(debug_assertions)]
        subtrie.debug_assert_counters();
        self.upper_arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);
    }

    /// If the cursor head is a branch, wraps any revealed children that sit at
    /// the subtrie boundary depth (`UPPER_TRIE_MAX_DEPTH`). This is needed after
    /// structural changes like root-level splits or subtrie unwraps that can place
    /// non-subtrie nodes at the boundary depth.
    fn maybe_wrap_branch_children(&mut self, cursor: &ArenaCursor) {
        let head_idx = cursor.head().expect("cursor is non-empty").index;
        let head_path = cursor.head_path();

        let ArenaSparseNode::Branch(b) = &self.upper_arena[head_idx] else { return };
        let short_key = b.short_key;
        let children: SmallVec<[_; 4]> = b
            .child_iter()
            .filter_map(|(nibble, child)| Some((nibble, child.revealed_index()?)))
            .collect();

        for (nibble, child_idx) in children {
            let mut child_path = head_path;
            child_path.extend(&short_key);
            child_path.push_unchecked(nibble);
            self.maybe_wrap_in_subtrie(child_idx, &child_path);
        }
    }

    /// Checks whether the subtrie at the cursor head has become empty after updates.
    /// If the subtrie's root is [`ArenaSparseNode::EmptyRoot`] (all leaves were removed), the
    /// child slot is removed from the parent branch entirely, the subtrie is recycled, and
    /// if the parent is left with a single revealed child, it is collapsed via
    /// `collapse_branch`.
    ///
    /// The subtrie must be the cursor head and its parent the cursor's parent.
    /// Pops the subtrie entry (propagating leaf count deltas) before returning.
    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(subtrie_path = ?cursor.head_path()),
    )]
    fn maybe_unwrap_subtrie(&mut self, cursor: &mut ArenaCursor) {
        let subtrie_idx = cursor.head().expect("cursor is non-empty").index;

        let ArenaSparseNode::Subtrie(subtrie) = &self.upper_arena[subtrie_idx] else {
            return;
        };

        if !matches!(subtrie.arena[subtrie.root], ArenaSparseNode::EmptyRoot { .. }) {
            return;
        }

        let child_nibble =
            cursor.head_last_nibble().expect("subtrie path must have at least one nibble");
        let parent_idx = cursor.parent().expect("cursor has parent").index;

        // Pop the subtrie entry before mutating, so collapse_branch sees the parent as
        // the cursor head.
        cursor.pop(&mut self.upper_arena);

        self.recycle_subtrie_from_idx(subtrie_idx);

        trace!(target: TRACE_TARGET, "Unwrapping empty subtrie, removing child slot");
        let parent_branch = self.upper_arena[parent_idx].branch_mut();
        let child_idx = BranchChildIdx::new(parent_branch.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");

        parent_branch.children.remove(child_idx.get());
        parent_branch.unset_child_bit(child_nibble);
        // The branch structure changed (child removed), so any cached RLP is stale.
        parent_branch.state = parent_branch.state.to_dirty();

        self.maybe_collapse_or_remove_branch(cursor);
    }

    /// Merges buffered updates from a [`ArenaSparseNode::Subtrie`] and drops it.
    ///
    /// # Panics
    ///
    /// Panics if `node` is not a `Subtrie`.
    fn recycle_subtrie(&mut self, node: ArenaSparseNode) {
        let ArenaSparseNode::Subtrie(mut subtrie) = node else {
            unreachable!("recycle_subtrie called on non-Subtrie node")
        };
        Self::merge_subtrie_updates(&mut self.buffers.updates, &mut subtrie.buffers.updates);
    }

    /// Removes a [`ArenaSparseNode::Subtrie`] from the upper arena at `idx` and recycles it.
    fn recycle_subtrie_from_idx(&mut self, idx: Index) {
        let node = self.upper_arena.remove(idx).expect("subtrie exists in arena");
        self.recycle_subtrie(node);
    }

    /// Puts a subtrie that was taken for parallel processing back into its upper arena slot.
    ///
    /// The slot must still hold the [`ArenaSparseNode::TakenSubtrie`] placeholder: indices carry
    /// no generation, so a slot that was removed and handed out again would silently be
    /// overwritten.
    fn restore_taken_subtrie(&mut self, idx: Index, subtrie: Box<ArenaSparseSubtrie>) {
        debug_assert!(
            matches!(self.upper_arena[idx], ArenaSparseNode::TakenSubtrie),
            "taken subtrie slot {idx:?} was removed or reused before restoration",
        );
        self.upper_arena[idx] = ArenaSparseNode::Subtrie(subtrie);
    }

    /// Handles cascading structural changes on the branch at the cursor head after a child
    /// has been removed.
    ///
    /// Depending on the remaining child count:
    /// - **0 children**: the branch becomes `EmptyRoot` (if root) or is removed from its parent,
    ///   cascading upward.
    /// - **1 child**: collapses the branch into its sole child, unless that child is a
    ///   `TakenSubtrie` (deferred) or blinded. If the remaining child is an empty subtrie, it is
    ///   also removed, reducing to the 0-children case.
    /// - **2+ children**: nothing to do.
    fn maybe_collapse_or_remove_branch(&mut self, cursor: &mut ArenaCursor) {
        loop {
            let branch_idx = cursor.head().expect("cursor is non-empty").index;

            // Read-only phase: extract the count and remaining-child info we need before
            // mutating. All values here are Copy so the borrow is released.
            let count = {
                let ArenaSparseNode::Branch(b) = &self.upper_arena[branch_idx] else {
                    return;
                };
                b.state_mask.count_bits()
            };

            if count >= 2 {
                return;
            }

            if count == 0 {
                if branch_idx == self.root {
                    self.upper_arena[branch_idx] =
                        ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Dirty };
                    return;
                }
                // Remove the empty branch from its parent.
                let branch_nibble = cursor.head_last_nibble().expect("non-root branch");
                cursor.pop(&mut self.upper_arena);
                self.upper_arena.remove(branch_idx);
                let parent_idx = cursor.head().expect("cursor is non-empty").index;
                let parent_branch = self.upper_arena[parent_idx].branch_mut();
                let child_idx = BranchChildIdx::new(parent_branch.state_mask, branch_nibble)
                    .expect("child nibble not found in parent state_mask");
                parent_branch.children.remove(child_idx.get());
                parent_branch.unset_child_bit(branch_nibble);
                parent_branch.state = parent_branch.state.to_dirty();
                continue; // re-check the parent
            }

            // count == 1 — determine what kind of child remains.
            let (remaining_nibble, remaining_child_idx) = {
                let b = self.upper_arena[branch_idx].branch_ref();
                let nibble = b.state_mask.iter().next().expect("branch has at least one child");
                (nibble, b.children[0].revealed_index())
            };

            let Some(child_idx) = remaining_child_idx else {
                debug_assert!(false, "single remaining child is blinded — should have been caught by check_subtrie_collapse_needs_proof");
                return;
            };

            if matches!(self.upper_arena[child_idx], ArenaSparseNode::TakenSubtrie) {
                // Subtrie hasn't been restored yet; collapse is deferred to the
                // post-restore phase.
                return;
            }

            // Check if the remaining child is an empty subtrie that should also be removed.
            let is_empty_subtrie = matches!(
                &self.upper_arena[child_idx],
                ArenaSparseNode::Subtrie(s) if matches!(s.arena[s.root], ArenaSparseNode::EmptyRoot { .. })
            );

            if is_empty_subtrie {
                self.recycle_subtrie_from_idx(child_idx);
                let branch = self.upper_arena[branch_idx].branch_mut();
                branch.children.remove(0);
                branch.unset_child_bit(remaining_nibble);
                branch.state = branch.state.to_dirty();
                continue; // now count == 0, will be handled next iteration
            }

            // Normal collapse: the remaining child is a Leaf, Branch, or non-empty Subtrie.
            Self::collapse_branch(
                &mut self.upper_arena,
                cursor,
                &mut self.root,
                &mut self.buffers.updates,
            );

            // After collapse, the remaining child (now at cursor head) may be a
            // Subtrie whose path was shortened by the collapsed branch's prefix. Since
            // should_be_subtrie requires path_len == UPPER_TRIE_MAX_DEPTH and the collapse
            // made the path shorter, the subtrie is no longer eligible — unwrap it.
            let child_idx = cursor.head().expect("cursor is non-empty").index;
            if let ArenaSparseNode::Subtrie(_) = &self.upper_arena[child_idx] {
                let ArenaSparseNode::Subtrie(mut subtrie) =
                    mem::replace(&mut self.upper_arena[child_idx], ArenaSparseNode::TakenSubtrie)
                else {
                    unreachable!()
                };
                Self::migrate_nodes(
                    &mut self.upper_arena,
                    &mut subtrie.arena,
                    subtrie.root,
                    Some(child_idx),
                );
                Self::merge_subtrie_updates(
                    &mut self.buffers.updates,
                    &mut subtrie.buffers.updates,
                );

                // The migrated subtrie root may be a branch whose children now live in
                // the upper arena at or beyond the subtrie boundary depth. Re-wrap any
                // such children as subtries.
                self.maybe_wrap_branch_children(cursor);
            }
            return;
        }
    }

    /// Merges updates from a subtrie's buffer into the parent's buffer.
    /// Both `dst` and `src` must be `Some` when updates are being tracked.
    ///
    /// Source removals cancel destination insertions (and vice versa) so that
    /// updates accumulated across multiple `root()` calls within a single block
    /// stay consistent.
    fn merge_subtrie_updates(
        dst: &mut Option<SparseTrieUpdates>,
        src: &mut Option<SparseTrieUpdates>,
    ) {
        if let Some(dst_updates) = dst.as_mut() {
            let src_updates = src.as_mut().expect("updates are enabled");

            // Source insertions cancel destination removals.
            for path in src_updates.updated_nodes.keys() {
                dst_updates.removed_nodes.remove(path);
            }
            dst_updates.updated_nodes.extend(src_updates.updated_nodes.drain());

            // Source removals cancel destination insertions.
            for path in &src_updates.removed_nodes {
                dst_updates.updated_nodes.remove(path);
            }
            dst_updates.removed_nodes.extend(src_updates.removed_nodes.drain());
        }
    }

    /// Right-pads a nibble path with zeros and packs it into a [`B256`].
    fn nibbles_to_padded_b256(path: &Nibbles) -> B256 {
        let mut bytes = [0u8; 32];
        path.pack_to(&mut bytes);
        B256::from(bytes)
    }

    /// Returns the [`BranchNodeMasks`] for a branch based on the status of its children.
    fn get_branch_masks(arena: &NodeArena, branch: &ArenaSparseNodeBranch) -> BranchNodeMasks {
        let mut masks = BranchNodeMasks::default();

        for (nibble, child) in branch.child_iter() {
            let (hash_bit, tree_bit) = match child.revealed_index() {
                Some(child_idx) => {
                    let child = &arena[child_idx];
                    (child.hash_mask_bit(), child.tree_mask_bit())
                }
                None => (
                    branch.branch_masks.hash_mask.is_bit_set(nibble),
                    branch.branch_masks.tree_mask.is_bit_set(nibble),
                ),
            };

            masks.set_child_bits(nibble, hash_bit, tree_bit);
        }

        masks
    }

    /// Immutable traversal to find a leaf value at `full_path` starting from `root` in `arena`.
    /// `path_offset` is the number of nibbles already consumed from `full_path`.
    fn get_leaf_value_in_arena<'a>(
        arena: &'a NodeArena,
        mut current: Index,
        full_path: &Nibbles,
        mut path_offset: usize,
    ) -> Option<&'a Vec<u8>> {
        loop {
            match &arena[current] {
                ArenaSparseNode::EmptyRoot { .. } |
                ArenaSparseNode::TakenSubtrie |
                ArenaSparseNode::Free => return None,
                ArenaSparseNode::Leaf { key, value, .. } => {
                    let remaining = full_path.slice_unchecked(path_offset, full_path.len());
                    return (remaining == *key).then_some(value);
                }
                ArenaSparseNode::Branch(b) => {
                    let short_key = &b.short_key;
                    let logical_end = path_offset + short_key.len();
                    if full_path.len() <= logical_end ||
                        (!short_key.is_empty() &&
                            full_path.slice_unchecked(path_offset, logical_end) != *short_key)
                    {
                        return None;
                    }

                    let child_nibble = full_path.get_unchecked(logical_end);
                    let child_idx = BranchChildIdx::new(b.state_mask, child_nibble)?;
                    current = b.children[child_idx].revealed_index()?;
                    path_offset = logical_end + 1;
                }
                ArenaSparseNode::Subtrie(subtrie) => {
                    return Self::get_leaf_value_in_arena(
                        &subtrie.arena,
                        subtrie.root,
                        full_path,
                        path_offset,
                    );
                }
            }
        }
    }

    /// Immutable traversal from the given root in `arena`, following `full_path` to find a leaf.
    /// Returns whether the leaf exists or not, or an error if a blinded node is encountered or
    /// the value doesn't match.
    fn find_leaf_in_arena(
        arena: &NodeArena,
        mut current: Index,
        full_path: &Nibbles,
        mut path_offset: usize,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        loop {
            match &arena[current] {
                ArenaSparseNode::EmptyRoot { .. } |
                ArenaSparseNode::TakenSubtrie |
                ArenaSparseNode::Free => {
                    return Ok(LeafLookup::NonExistent);
                }
                ArenaSparseNode::Leaf { key, value, .. } => {
                    let remaining = full_path.slice(path_offset..);
                    if remaining != *key {
                        return Ok(LeafLookup::NonExistent);
                    }
                    if let Some(expected) = expected_value &&
                        *expected != *value
                    {
                        return Err(LeafLookupError::ValueMismatch {
                            path: *full_path,
                            expected: Some(expected.clone()),
                            actual: value.clone(),
                        });
                    }
                    return Ok(LeafLookup::Exists);
                }
                ArenaSparseNode::Branch(b) => {
                    let short_key = &b.short_key;
                    let logical_end = path_offset + short_key.len();

                    if full_path.len() <= logical_end {
                        return Ok(LeafLookup::NonExistent);
                    }

                    if full_path.slice(path_offset..logical_end) != *short_key {
                        return Ok(LeafLookup::NonExistent);
                    }

                    let child_nibble = full_path.get_unchecked(logical_end);
                    let Some(child_idx) = BranchChildIdx::new(b.state_mask, child_nibble) else {
                        return Ok(LeafLookup::NonExistent);
                    };

                    let child = b.children[child_idx];
                    match child.revealed_index() {
                        None => {
                            let rlp_node = arena.blinded(child);
                            let hash = rlp_node
                                .as_hash()
                                .unwrap_or_else(|| keccak256(rlp_node.as_slice()));
                            let mut blinded_path = full_path.slice(..logical_end);
                            blinded_path.push_unchecked(child_nibble);
                            return Err(LeafLookupError::BlindedNode { path: blinded_path, hash });
                        }
                        Some(child_idx) => {
                            current = child_idx;
                            path_offset = logical_end + 1;
                        }
                    }
                }
                ArenaSparseNode::Subtrie(subtrie) => {
                    return Self::find_leaf_in_arena(
                        &subtrie.arena,
                        subtrie.root,
                        full_path,
                        path_offset,
                        expected_value,
                    );
                }
            }
        }
    }

    /// Counts the total leaves and dirty leaves in a subtree rooted at `idx`.
    fn count_leaves_and_dirty(arena: &NodeArena, idx: Index) -> (u64, u64) {
        match &arena[idx] {
            ArenaSparseNode::Leaf { state, .. } => {
                let dirty = matches!(state, ArenaSparseNodeState::Dirty) as u64;
                (1, dirty)
            }
            ArenaSparseNode::Branch(b) => {
                let mut leaves = 0u64;
                let mut dirty = 0u64;
                for c in &b.children {
                    if let Some(child_idx) = c.revealed_index() {
                        let (l, d) = Self::count_leaves_and_dirty(arena, child_idx);
                        leaves += l;
                        dirty += d;
                    }
                }
                (leaves, dirty)
            }
            _ => (0, 0),
        }
    }

    /// Asserts that every node in the upper arena satisfies the subtrie structure invariant:
    /// - Nodes at `UPPER_TRIE_MAX_DEPTH` path length must be `Subtrie` (or `TakenSubtrie`).
    /// - Nodes at other depths must NOT be `Subtrie`.
    ///
    /// Uses the cursor to DFS the upper arena, checking each visited node's path length.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    #[cfg(debug_assertions)]
    fn debug_assert_subtrie_structure(&mut self) {
        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        loop {
            let result = cursor.next(&mut self.upper_arena, |_, _| true);
            match result {
                NextResult::Done => break,
                NextResult::NonBranch | NextResult::Branch => {
                    let path_len = cursor.head_path_len();
                    let node = &self.upper_arena[cursor.head().expect("cursor is non-empty").index];

                    if Self::should_be_subtrie(path_len) {
                        debug_assert!(
                            matches!(
                                node,
                                ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie
                            ),
                            "node at path_len={path_len} should be a Subtrie but is {node:?}",
                        );
                    } else {
                        debug_assert!(
                            !matches!(node, ArenaSparseNode::Subtrie(_)),
                            "node at path_len={path_len} should NOT be a Subtrie but is",
                        );
                    }
                }
            }
        }

        self.buffers.cursor = cursor;
    }

    /// Recursively migrates all nodes from `src` into `dst`, starting at `src_idx`.
    /// Branch children's `Revealed` indices are remapped to the new `dst` indices during
    /// the migration.
    ///
    /// If `dst_slot` is `Some(idx)`, the node at `src_idx` is placed into `dst[idx]`
    /// (overwriting); otherwise a new slot is allocated. Returns the `dst` index of the
    /// migrated node.
    fn migrate_nodes(
        dst: &mut NodeArena,
        src: &mut NodeArena,
        src_idx: Index,
        dst_slot: Option<Index>,
    ) -> Index {
        let mut node = src.remove(src_idx).expect("node exists in source arena");

        // Recursively migrate children first so their new indices are known.
        Self::migrate_children(dst, src, &mut node);

        if let Some(slot) = dst_slot {
            dst[slot] = node;
            slot
        } else {
            dst.insert(node)
        }
    }

    /// Moves a branch's children from `src` to `dst`: revealed children are migrated recursively
    /// with [`Self::migrate_nodes`], blinded children have their RLP moved into `dst`'s side
    /// table. `node` must already be detached from `src`; non-branch nodes are left untouched.
    fn migrate_children(dst: &mut NodeArena, src: &mut NodeArena, node: &mut ArenaSparseNode) {
        let ArenaSparseNode::Branch(b) = node else { return };
        for child in &mut b.children {
            *child = match child.revealed_index() {
                Some(child_idx) => {
                    BranchChild::revealed(Self::migrate_nodes(dst, src, child_idx, None))
                }
                None => dst.insert_blinded(src.take_blinded(*child)),
            };
        }
    }

    /// Removes a pruned node from the arena and blinds the parent's child slot with the node's
    /// cached RLP.
    fn remove_pruned_node(arena: &mut NodeArena, cursor: &mut ArenaCursor) -> ArenaSparseNode {
        // Entry paths are derived from the cursor's path, so read the head path before popping.
        let path = cursor.head_path();
        let entry = cursor.pop(arena);
        let node = arena.remove(entry.index).expect("node must exist to be pruned");
        let rlp_node = node
            .state_ref()
            .and_then(ArenaSparseNodeState::cached_rlp_node)
            .cloned()
            .expect("prune must run after hashing");
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
        let parent_branch = arena[parent_idx].branch_mut();
        let child_idx = BranchChildIdx::new(parent_branch.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");
        parent_branch.children[child_idx] = blinded;

        node
    }

    /// Reveals a single proof node using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// If the result is `Blinded`, the blinded child is replaced with the proof node (converted to
    /// an arena node with `Cached` state). All other cases (already revealed, no child, diverged,
    /// leaf head) are no-ops — the proof node is skipped.
    ///
    /// Returns the `Index` of the revealed node in the arena, if any was revealed.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn reveal_node(
        arena: &mut NodeArena,
        cursor: &ArenaCursor,
        node: &mut ProofTrieNodeV2,
        find_result: SeekResult,
    ) -> Option<Index> {
        let SeekResult::Blinded = find_result else {
            // Already revealed, no child slot, or diverged — skip this proof node.
            return None;
        };

        let head = cursor.head().expect("cursor is non-empty");
        let head_idx = head.index;
        let head_branch_logical_path = cursor.head_logical_branch_path(arena);

        debug_assert_eq!(
            node.path.len(),
            head_branch_logical_path.len() + 1,
            "proof node path {:?} is not a direct child of branch at {:?} (expected depth {})",
            node.path,
            head_branch_logical_path,
            head_branch_logical_path.len() + 1,
        );

        let child_nibble = node.path.get_unchecked(head_branch_logical_path.len());
        let head_branch = arena[head_idx].branch_ref();
        let dense_child_idx = BranchChildIdx::new(head_branch.state_mask, child_nibble)
            .expect("Blinded result but child nibble not in state_mask");

        let child = head_branch.children[dense_child_idx];
        if !child.is_blinded() {
            return None;
        }
        let cached_rlp = arena.blinded(child).clone();

        trace!(
            target: TRACE_TARGET,
            path = ?node.path,
            rlp_node = ?cached_rlp,
            "Revealing node",
        );

        let proof_node = mem::replace(node, ProofTrieNodeV2::empty());
        let mut arena_node = ArenaSparseNode::from_proof_node(arena, proof_node);

        let state = arena_node.state_mut();
        *state =
            ArenaSparseNodeState::Cached { rlp_node: cached_rlp, epoch: TrieNodeEpoch::UNMODIFIED };

        let child_idx = arena.insert(arena_node);
        arena[head_idx].branch_mut().children[dense_child_idx] = BranchChild::revealed(child_idx);
        arena.take_blinded(child);

        Some(child_idx)
    }

    #[cfg(debug_assertions)]
    fn collect_reachable_nodes(
        arena: &NodeArena,
        idx: Index,
        reachable: &mut alloy_primitives::map::HashSet<Index>,
    ) {
        if !reachable.insert(idx) {
            return;
        }
        if let ArenaSparseNode::Branch(b) = &arena[idx] {
            for child in &b.children {
                if let Some(child_idx) = child.revealed_index() {
                    Self::collect_reachable_nodes(arena, child_idx, reachable);
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    fn assert_no_orphaned_nodes(arena: &NodeArena, root: Index, label: &str) {
        let mut reachable = alloy_primitives::map::HashSet::default();
        Self::collect_reachable_nodes(arena, root, &mut reachable);
        let all_indices: alloy_primitives::map::HashSet<Index> =
            arena.iter().map(|(idx, _)| idx).collect();
        let orphaned: Vec<_> = all_indices.difference(&reachable).collect();
        debug_assert!(
            orphaned.is_empty(),
            "{label} has {} orphaned node(s): {orphaned:?}",
            orphaned.len(),
        );
    }
}

#[cfg(debug_assertions)]
impl Drop for ArenaParallelSparseTrie {
    fn drop(&mut self) {
        Self::assert_no_orphaned_nodes(&self.upper_arena, self.root, "upper arena");

        for (_, node) in self.upper_arena.iter() {
            if let Some(subtrie) = node.as_subtrie() {
                Self::assert_no_orphaned_nodes(
                    &subtrie.arena,
                    subtrie.root,
                    &alloc::format!("subtrie {:?}", subtrie.path),
                );
            }
        }
    }
}

impl Default for ArenaParallelSparseTrie {
    fn default() -> Self {
        let mut upper_arena = NodeArena::new();
        let root = upper_arena
            .insert(ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Revealed });
        Self {
            upper_arena,
            root,
            buffers: ArenaTrieBuffers::default(),
            parallelism_thresholds: ArenaParallelismThresholds::default(),
        }
    }
}

impl ArenaParallelSparseTrie {
    /// Hashes a subtrie at `head_idx` and collects its update actions.
    fn update_upper_subtrie(&mut self, head_idx: Index, new_epoch: TrieNodeEpoch) {
        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx] else {
            unreachable!()
        };

        if !subtrie.arena[subtrie.root].is_cached() {
            subtrie.update_cached_rlp(new_epoch);
        }

        Self::merge_subtrie_updates(&mut self.buffers.updates, &mut subtrie.buffers.updates);
    }
}

impl SparseTrie for ArenaParallelSparseTrie {
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

        self.set_updates(retain_updates);

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
                trace!(target: TRACE_TARGET, state_mask = ?branch.state_mask(), num_children = branch.state_mask().count_bits(), "Setting branch root");
                let mut children =
                    SmallVec::with_capacity(branch.state_mask().count_bits() as usize);
                for (stack_ptr, _nibble) in branch.state_mask().iter().enumerate() {
                    let child = self.upper_arena.insert_blinded(branch.stack()[stack_ptr].clone());
                    children.push(child);
                }

                self.upper_arena[self.root] = ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed,
                    children,
                    state_mask: branch.state_mask(),
                    short_key: *branch.key(),
                    branch_masks: masks.unwrap_or_default(),
                });
            }
            TrieNodeV2::Extension(node) => {
                return Err(SparseTrieErrorKind::Reveal {
                    path: Nibbles::new(),
                    node: Box::new(node),
                }
                .into())
            }
        }

        Ok(())
    }

    fn set_updates(&mut self, retain_updates: bool) {
        if retain_updates {
            self.buffers.updates.get_or_insert_with(SparseTrieUpdates::default);
        } else {
            self.buffers.updates = None;
        }
        for (_, node) in self.upper_arena.iter_mut() {
            if let ArenaSparseNode::Subtrie(subtrie) = node {
                if retain_updates {
                    subtrie.buffers.updates.get_or_insert_with(SparseTrieUpdates::default);
                } else {
                    subtrie.buffers.updates = None;
                }
            }
        }
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
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>, Vec<ProofTrieNodeV2>)> = Vec::new();

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
                        let mut subtrie_nodes: Vec<ProofTrieNodeV2> = (subtrie_start..node_idx)
                            .map(|i| mem::replace(&mut nodes[i], ProofTrieNodeV2::empty()))
                            .collect();
                        subtrie.reveal_nodes(&mut subtrie_nodes)?;
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
            subtrie.reveal_nodes(node_vec)?;
        } else {
            use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

            let parent_span = tracing::Span::current();
            let results: Vec<SparseTrieResult<()>> = taken
                .par_iter_mut()
                .map(|(_, subtrie, node_vec)| {
                    let _guard = parent_span.enter();
                    subtrie.reveal_nodes(node_vec)
                })
                .collect();

            if let Some(err) = results.into_iter().find(|r| r.is_err()) {
                // Restore before returning so we don't leave TakenSubtrie holes.
                for (idx, subtrie, _) in taken {
                    self.restore_taken_subtrie(idx, subtrie);
                }
                return err;
            }
        }

        // Restore taken subtries into the upper arena.
        for (idx, subtrie, _) in taken {
            self.restore_taken_subtrie(idx, subtrie);
        }

        #[cfg(debug_assertions)]
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
        self.upper_arena[self.root].is_cached()
    }

    fn root_epoch(&self) -> Option<TrieNodeEpoch> {
        match self.upper_arena[self.root].state_ref()? {
            ArenaSparseNodeState::Revealed => Some(TrieNodeEpoch::UNMODIFIED),
            ArenaSparseNodeState::Cached { epoch, .. } => Some(*epoch),
            ArenaSparseNodeState::Dirty => None,
        }
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn update_subtrie_hashes(&mut self, new_epoch: TrieNodeEpoch) {
        trace!(target: TRACE_TARGET, "Updating subtrie hashes");

        // Only descend if the root is a branch; otherwise there are no subtries.
        if !matches!(&self.upper_arena[self.root], ArenaSparseNode::Branch(_)) {
            return;
        }

        // Count total dirty leaves across all subtries to make one global parallelism decision.
        let mut total_dirty_leaves: u64 = 0;
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>)> = Vec::new();
        for (idx, node) in self.upper_arena.iter_mut() {
            let ArenaSparseNode::Subtrie(s) = node else { continue };
            if s.num_dirty_leaves == 0 {
                continue;
            }
            total_dirty_leaves += s.num_dirty_leaves;
            let ArenaSparseNode::Subtrie(subtrie) =
                mem::replace(node, ArenaSparseNode::TakenSubtrie)
            else {
                unreachable!()
            };
            taken.push((idx, subtrie));
        }

        // Hash taken subtries in parallel if total dirty leaves meet the threshold.
        if !taken.is_empty() {
            if taken.len() == 1 || total_dirty_leaves < self.parallelism_thresholds.min_dirty_leaves
            {
                for (_, subtrie) in &mut taken {
                    subtrie.update_cached_rlp(new_epoch);
                }
            } else {
                use rayon::iter::{IntoParallelIterator, ParallelIterator};

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
        if taken.is_empty() && self.upper_arena[self.root].is_cached() {
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
                ArenaSparseNode::TakenSubtrie => true,
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

        // Only descend if the root is a branch; otherwise there are no subtries.
        if !matches!(&self.upper_arena[self.root], ArenaSparseNode::Branch(_)) {
            return 0;
        }

        let threshold = self.parallelism_thresholds.min_leaves_for_prune;

        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // Subtries taken for parallel pruning.
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>)> = Vec::new();

        let mut pruned = 0;

        loop {
            let result = cursor.next(&mut self.upper_arena, |_, child| {
                matches!(
                    child,
                    ArenaSparseNode::Branch(_) |
                        ArenaSparseNode::Subtrie(_) |
                        ArenaSparseNode::Leaf { .. }
                )
            });

            if matches!(result, NextResult::Done) {
                break
            }

            let head_idx = cursor.head().expect("cursor is non-empty").index;

            match &self.upper_arena[head_idx] {
                ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. } => {
                    // Don't prune the root.
                    if cursor.depth() == 0 {
                        continue;
                    }

                    let node_epoch = self.upper_arena[head_idx]
                        .state_ref()
                        .and_then(ArenaSparseNodeState::cached_epoch)
                        .expect("prune must run after hashing");
                    if !node_epoch.should_prune(prune_before) {
                        continue;
                    }

                    Self::remove_pruned_node(&mut self.upper_arena, &mut cursor);
                    pruned += 1;
                }
                ArenaSparseNode::Subtrie(_) => {
                    let root_epoch = self.upper_arena[head_idx]
                        .state_ref()
                        .and_then(ArenaSparseNodeState::cached_epoch)
                        .expect("prune must run after hashing");
                    if root_epoch.should_prune(prune_before) {
                        let removed = Self::remove_pruned_node(&mut self.upper_arena, &mut cursor);
                        let ArenaSparseNode::Subtrie(s) = &removed else { unreachable!() };
                        pruned += s.arena.len();
                        self.recycle_subtrie(removed);
                        continue;
                    }

                    let ArenaSparseNode::Subtrie(subtrie) = &self.upper_arena[head_idx] else {
                        unreachable!()
                    };
                    if subtrie.num_leaves >= threshold {
                        let ArenaSparseNode::Subtrie(subtrie) = mem::replace(
                            &mut self.upper_arena[head_idx],
                            ArenaSparseNode::TakenSubtrie,
                        ) else {
                            unreachable!()
                        };
                        taken.push((head_idx, subtrie));
                    } else {
                        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx]
                        else {
                            unreachable!()
                        };
                        pruned += subtrie.prune(prune_before);
                    }
                }
                _ => unreachable!("NonBranch in prune walk must be Subtrie, Leaf, or Branch"),
            }
        }

        self.buffers.cursor = cursor;

        if !taken.is_empty() {
            // Prune taken subtries, in parallel if more than one.
            if taken.len() == 1 {
                let (_, ref mut subtrie) = taken[0];
                pruned += subtrie.prune(prune_before);
            } else {
                use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

                let parent_span = tracing::Span::current();
                pruned += taken
                    .par_iter_mut()
                    .map(|(_, subtrie)| {
                        let _guard = parent_span.enter();
                        let _span = tracing::trace_span!(
                            target: TRACE_TARGET,
                            "subtrie_prune",
                            subtrie = ?subtrie.path,
                        )
                        .entered();

                        subtrie.prune(prune_before)
                    })
                    .sum::<usize>();
            }

            // Restore taken subtries into the upper arena.
            for (child_idx, subtrie) in taken {
                self.restore_taken_subtrie(child_idx, subtrie);
            }
        }

        if pruned > 0 {
            compact_arena(&mut self.upper_arena, &mut self.root);
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
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>, core::ops::Range<usize>)> = Vec::new();

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
                        // Pop the subtrie entry before continuing.
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
            #[cfg(debug_assertions)]
            self.debug_assert_subtrie_structure();

            return Ok(());
        }

        // Apply updates to taken subtries, in parallel if more than one.
        if taken.len() == 1 {
            let (_, ref mut subtrie, ref range) = taken[0];
            subtrie.update_leaves(&sorted[range.clone()]);
        } else {
            use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

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

        #[cfg(debug_assertions)]
        self.debug_assert_subtrie_structure();

        Ok(())
    }
}

#[cfg(test)]
mod tests;
