//! Persistent parallel arena implementation of the sparse trie.
//!
//! ## Structure
//!
//! Uses arena allocation (`NodeArena`) for node storage with direct index-based child
//! pointers, avoiding the per-node hashing overhead of a `HashMap`-based trie. The trie is split
//! into two tiers:
//!
//! - **Upper trie** (`upper_arena`): Contains nodes whose path is shorter than
//!   `UPPER_TRIE_MAX_DEPTH` nibbles. These are the root and its immediate children.
//! - **Lower subtries** (`ArenaSparseSubtrie`): Each child of an upper-trie branch at the depth
//!   boundary becomes the root of its own subtrie, stored as an `ArenaSparseNode::Subtrie` child in
//!   the upper arena. Each subtrie owns its own arena, enabling lock-free parallel mutation.
//!
//! Node placement is determined by path length (not counting a branch's short key):
//!
//! - Paths with **< `UPPER_TRIE_MAX_DEPTH`** nibbles live in `upper_arena`.
//! - Paths with **≥ `UPPER_TRIE_MAX_DEPTH`** nibbles live in a subtrie.
//!
//! ## Node Revealing
//!
//! Nodes are lazily revealed from proof data via
//! [`SparseTrie::reveal_nodes`](crate::SparseTrie::reveal_nodes). Each node is placed into the
//! upper arena or delegated to its subtrie based on path depth. Unrevealed children are stored as
//! blinded `BranchChild`s referencing their RLP encoding. Extensions keep their own cache and
//! retain an opaque branch reference inline until the backing branch proof arrives, without
//! allocating a placeholder arena slot. When multiple subtries have pending reveals, they are
//! processed in parallel using rayon (controlled by
//! [`PersistentArenaParallelismThresholds::min_revealed_nodes`]).
//!
//! ## Leaf Operations
//!
//! Leaf updates and removals are applied via
//! [`SparseTrie::update_leaves`](crate::SparseTrie::update_leaves). The method walks the upper trie
//! to route each update to the correct subtrie, then processes eligible subtries in parallel
//! according to [`PersistentArenaParallelismThresholds::min_updates`].
//!
//! After updates, structural changes (branch collapse, subtrie unwrapping) are handled by
//! propagating dirty state back up through the upper trie.
//!
//! ## Root Hash Calculation
//!
//! Root hash computation follows a bottom-up approach:
//!
//! 1. **`update_subtrie_hashes`**: Takes dirty subtries from the upper arena and hashes them in
//!    parallel (when the total dirty leaf count meets
//!    [`PersistentArenaParallelismThresholds::min_dirty_leaves`]), then walks the upper trie to
//!    restore hashed subtries and inline-hash any remaining dirty nodes.
//! 2. **[`SparseTrie::root`](crate::SparseTrie::root)**: Calls `update_subtrie_hashes`, then
//!    RLP-encodes the full upper trie depth-first to produce the root hash.
//!
//! Each node tracks its state via `ArenaSparseNodeState` (`Revealed`, `Cached`, or `Dirty`)
//! so only modified subtrees are recomputed.
//!
//! ## Snapshots and Pruning
//!
//! Snapshots share the backing slot tables and copy only the paths to changed slots. Each
//! version can update and recycle its own slots without affecting another version. Pruning
//! replaces old revealed nodes with their cached encodings. It can compact exclusive storage,
//! but must preserve slot indices when another snapshot still shares that storage.

mod branch_child_idx;
mod cursor;
mod leaves;
mod node_arena;
mod nodes;
mod subtrie;
mod trie;

// Require an explicit cfg: ordinary `--all-features` Clippy, udeps, doctests and private
// docs must not instantiate the 28,674-function coverage bank. Enable explicitly with
// `RUSTFLAGS="--cfg reth_sparse_trie_coverage"`; the `fuzz` feature is not required.
#[cfg(reth_sparse_trie_coverage)]
mod sparse_guidance;

use branch_child_idx::{BranchChildIdx, BranchChildIter};
use cursor::{branch_idx, ArenaCursor, NextResult, SeekResult};
use node_arena::{BranchChild, NodeArena, NodeIndex};
use nodes::{ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeState, ExtensionChild};
use subtrie::ArenaSparseSubtrie;

use crate::{LeafLookup, LeafLookupError, LeafUpdate, SparseTrieUpdates, TrieNodeEpoch};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::{keccak256, B256};
use core::mem;
use reth_trie_common::{
    BranchNodeMasks, BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, ProofTrieNode,
    ProofV2TargetParent, RlpNode, TrieNode, EMPTY_ROOT_HASH,
};
use smallvec::SmallVec;
use tracing::{instrument, trace};

const TRACE_TARGET: &str = "trie::arena_persistent";

/// The maximum path length (in nibbles) for nodes that live in the upper trie. Nodes at this
/// depth or deeper belong to lower subtries.
const UPPER_TRIE_MAX_DEPTH: usize = 2;

/// Returns the branch data for a branch or extension node.
fn branch_data(arena: &NodeArena, idx: NodeIndex) -> &ArenaSparseNodeBranch {
    arena[branch_idx(arena, idx).expect("node must be a branch or extension")].branch_ref()
}

/// Returns the mutable branch data for a branch or extension node.
fn branch_data_mut(arena: &mut NodeArena, idx: NodeIndex) -> &mut ArenaSparseNodeBranch {
    let idx = branch_idx(arena, idx).expect("node must be a branch or extension");
    arena[idx].branch_mut()
}

/// Returns the cached or dirty state for a node, following subtrie roots.
fn node_state_ref(arena: &NodeArena, idx: NodeIndex) -> Option<&ArenaSparseNodeState> {
    arena[idx].state_ref()
}

/// Returns mutable cached or dirty state for a branch, extension, leaf, or empty root node.
fn node_state_mut(arena: &mut NodeArena, idx: NodeIndex) -> &mut ArenaSparseNodeState {
    arena[idx].state_mut()
}

/// Returns whether a node should contribute a bit in its parent's tree mask.
fn node_tree_mask_bit(arena: &NodeArena, node: &ArenaSparseNode) -> bool {
    match node {
        ArenaSparseNode::Branch(branch) => !branch.branch_masks.is_empty(),
        ArenaSparseNode::Extension { child: ExtensionChild::Revealed(child), .. } => {
            !arena[*child].branch_ref().branch_masks.is_empty()
        }
        ArenaSparseNode::Subtrie(subtrie) => {
            node_tree_mask_bit(&subtrie.arena, &subtrie.arena[subtrie.root])
        }
        _ => false,
    }
}

/// Returns whether an extension (possibly a subtrie root) has an opaque backing branch.
fn has_blinded_branch(arena: &NodeArena, idx: NodeIndex) -> bool {
    match &arena[idx] {
        ArenaSparseNode::Extension { child, .. } => matches!(child, ExtensionChild::Blinded(_)),
        ArenaSparseNode::Subtrie(s) => has_blinded_branch(&s.arena, s.root),
        _ => false,
    }
}

/// Starts opaque-branch proofs above the extension rather than claiming its branch is revealed.
fn blinded_proof_parent(arena: &NodeArena, cursor: &ArenaCursor) -> ProofV2TargetParent {
    if has_blinded_branch(arena, cursor.head().expect("cursor is non-empty").index) {
        cursor
            .head_path_len()
            .checked_sub(1)
            .map_or(ProofV2TargetParent::NONE, ProofV2TargetParent::new)
    } else {
        ProofV2TargetParent::new(cursor.head_logical_branch_path_len(arena))
    }
}

/// Structural edits require the opaque backing branch's persistence masks before splitting.
fn split_needs_proof(
    arena: &NodeArena,
    cursor: &ArenaCursor,
    result: &SeekResult,
    update: &LeafUpdate,
) -> bool {
    matches!(result, SeekResult::Diverged) &&
        matches!(update, LeafUpdate::Changed(value) if !value.is_empty()) &&
        has_blinded_branch(arena, cursor.head().expect("cursor is non-empty").index)
}

/// A surviving opaque extension needs its backing masks before collapsing its parent.
fn child_needs_collapse_proof(arena: &NodeArena, child: BranchChild) -> bool {
    child.revealed_index().is_none_or(|idx| has_blinded_branch(arena, idx))
}

/// Selects a target below an opaque extension rather than an exclusion outside its key.
fn extend_blinded_branch_path(arena: &NodeArena, child: BranchChild, path: &mut Nibbles) {
    if let Some(idx) = child.revealed_index() {
        match &arena[idx] {
            ArenaSparseNode::Extension { key, .. } => path.extend(key),
            ArenaSparseNode::Subtrie(s) => {
                if let Some(key) = s.arena[s.root].extension_key() {
                    path.extend(key);
                }
            }
            _ => {}
        }
    }
}

/// Reusable traversal state and optional accumulators shared by
/// [`ArenaSparseSubtrie`] and [`PersistentArenaParallelSparseTrie`].
#[derive(Debug, Default)]
struct ArenaTrieBuffers {
    /// Reusable cursor for trie traversals.
    cursor: ArenaCursor,
    /// Trie updates built up directly during hashing and structural changes. `Some` when
    /// tracking updates, `None` otherwise.
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

    fn reset_updates(&mut self, retain: bool) {
        if retain {
            self.updates.get_or_insert_with(SparseTrieUpdates::default).clear();
        } else {
            self.updates = None;
        }
    }
}

impl Clone for ArenaTrieBuffers {
    fn clone(&self) -> Self {
        // Traversals reset the cursor and encoders clear their scratch before use.
        Self { updates: self.updates.clone(), ..Self::default() }
    }
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
    /// Branch collapse requires revealing a blinded sibling before retrying the removal.
    NeedsProof {
        /// Leaf key whose removal must be retried.
        key: B256,
        /// Padded path of the blinded sibling to request a proof for.
        proof_key: B256,
        /// Revealed parent of the blinded sibling.
        parent: ProofV2TargetParent,
    },
}

/// A proof request generated during leaf updates when a blinded node is encountered.
#[derive(Debug, Clone)]
struct ArenaRequiredProof {
    /// The key requiring a proof.
    key: B256,
    /// The revealed logical parent branch.
    parent: ProofV2TargetParent,
}

/// Configuration for controlling when parallelism is enabled in
/// [`PersistentArenaParallelSparseTrie`] operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistentArenaParallelismThresholds {
    /// Minimum total number of dirty leaves across subtries to hash those subtries in parallel.
    pub min_dirty_leaves: u64,
    /// Minimum number of nodes to reveal in a subtrie before it is eligible for parallel
    /// reveal. Subtries with fewer nodes to reveal than this are revealed inline during the
    /// upper trie walk.
    pub min_revealed_nodes: usize,
    /// Minimum updates per subtrie for parallel processing. When the whole batch has at least
    /// four times this many updates, smaller subtrie batches also qualify. Batches that could
    /// empty their subtrie still run inline.
    pub min_updates: usize,
    /// Minimum number of revealed leaves in a subtrie before it is eligible for parallel
    /// pruning. Subtries with fewer leaves than this are pruned inline during the upper trie
    /// walk.
    pub min_leaves_for_prune: u64,
}

impl Default for PersistentArenaParallelismThresholds {
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
/// Use [`SparseTrie::snapshot`](crate::SparseTrie::snapshot) to take an independently writable copy
/// that shares unchanged nodes. A snapshot copies pending updates but does not copy revealed
/// leaves. It is most useful when both copies stay live and modify only part of the trie.
/// [`Clone::clone`] instead copies all node data independently.
/// Persistence here means sharing state between in-memory versions, not writing it to disk.
///
/// Pruning preserves the state root. A live snapshot can keep pruned nodes in memory. The first
/// write after other snapshots drop may take longer because it can reclaim superseded data.
#[derive(Debug)]
pub struct PersistentArenaParallelSparseTrie {
    /// The arena allocating nodes in the upper trie.
    upper_arena: NodeArena,
    /// The root node of the upper trie.
    root: NodeIndex,
    /// Reusable traversal and RLP scratch, plus an optional trie update accumulator.
    buffers: ArenaTrieBuffers,
    /// Thresholds controlling when parallelism is enabled for different operations.
    parallelism_thresholds: PersistentArenaParallelismThresholds,
}

impl PersistentArenaParallelSparseTrie {
    /// Sets the thresholds that control when parallelism is used during operations.
    pub const fn with_parallelism_thresholds(
        mut self,
        thresholds: PersistentArenaParallelismThresholds,
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
    fn maybe_wrap_in_subtrie(&mut self, child_idx: NodeIndex, child_path: &Nibbles) {
        if !Self::should_be_subtrie(child_path.len()) {
            return;
        }

        if !matches!(
            self.upper_arena[child_idx],
            ArenaSparseNode::Branch(_) |
                ArenaSparseNode::Extension { .. } |
                ArenaSparseNode::Leaf { .. }
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
        #[cfg(all(debug_assertions, not(feature = "fuzz")))]
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

        let Some(branch_idx) = branch_idx(&self.upper_arena, head_idx) else { return };
        let short_key = self.upper_arena[head_idx].extension_key().copied();
        let b = self.upper_arena[branch_idx].branch_ref();
        let children: SmallVec<[_; 4]> = b
            .child_iter()
            .filter_map(|(nibble, child)| Some((nibble, child.revealed_index()?)))
            .collect();

        for (nibble, child_idx) in children {
            let mut child_path = head_path;
            if let Some(key) = short_key {
                child_path.extend(&key);
            }
            child_path.push_unchecked(nibble);
            self.maybe_wrap_in_subtrie(child_idx, &child_path);
        }
    }

    /// Checks whether the subtrie at the cursor head has become empty after updates.
    /// If the subtrie's root is [`ArenaSparseNode::EmptyRoot`] (all leaves were removed), the
    /// child slot is removed from the parent branch entirely, the subtrie is dropped, and
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
        branch_data_mut(&mut self.upper_arena, parent_idx).remove_child(child_nibble);

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
    fn recycle_subtrie_from_idx(&mut self, idx: NodeIndex) {
        let node = self.upper_arena.remove(idx).expect("subtrie exists in arena");
        self.recycle_subtrie(node);
    }

    /// Puts a subtrie that was taken for parallel processing back into its upper arena slot.
    ///
    /// The slot must still hold the [`ArenaSparseNode::TakenSubtrie`] placeholder: indices carry
    /// no generation, so a slot that was removed and handed out again would silently be
    /// overwritten.
    fn restore_taken_subtrie(&mut self, idx: NodeIndex, subtrie: Box<ArenaSparseSubtrie>) {
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
            let branch_node_idx = cursor.head().expect("cursor is non-empty").index;
            let branch_path = cursor.head_path();

            // Read-only phase: extract the count and remaining-child info we need before
            // mutating. All values here are Copy so the borrow is released.
            let Some(branch_child_idx) = branch_idx(&self.upper_arena, branch_node_idx) else {
                return;
            };
            let count = self.upper_arena[branch_child_idx].branch_ref().state_mask.count_bits();

            if count >= 2 {
                return;
            }

            if count == 0 {
                if branch_node_idx == self.root {
                    self.upper_arena[branch_node_idx] =
                        ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Dirty };
                    if branch_child_idx != branch_node_idx {
                        self.upper_arena.discard(branch_child_idx);
                    }
                    return;
                }
                // Remove the empty branch from its parent.
                let branch_nibble = branch_path.last().expect("non-root branch");
                cursor.pop(&mut self.upper_arena);
                self.upper_arena.discard(branch_node_idx);
                if branch_child_idx != branch_node_idx {
                    self.upper_arena.discard(branch_child_idx);
                }
                let parent_idx = cursor.head().expect("cursor is non-empty").index;
                branch_data_mut(&mut self.upper_arena, parent_idx).remove_child(branch_nibble);
                continue; // re-check the parent
            }

            // count == 1 — determine what kind of child remains.
            let (remaining_nibble, remaining_child_idx) = {
                let b = self.upper_arena[branch_child_idx].branch_ref();
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
                branch_data_mut(&mut self.upper_arena, branch_node_idx)
                    .remove_child(remaining_nibble);
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

    /// Computes persistence masks, preserving the existing tree bit for opaque extensions.
    fn get_branch_masks(arena: &NodeArena, branch: &ArenaSparseNodeBranch) -> BranchNodeMasks {
        let mut masks = BranchNodeMasks::default();

        for (nibble, child) in branch.child_iter() {
            let (hash_bit, tree_bit) = match child.revealed_index() {
                Some(child_idx) => {
                    let child = &arena[child_idx];
                    (
                        child.hash_mask_bit(),
                        if has_blinded_branch(arena, child_idx) {
                            branch.branch_masks.tree_mask.is_bit_set(nibble)
                        } else {
                            node_tree_mask_bit(arena, child)
                        },
                    )
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

    /// Computes and caches `RlpNode` for all dirty nodes reachable from `root` in `arena`.
    ///
    /// Walks uncached branches in post-order. For each branch, the cursor visits uncached
    /// child branches first. Then the encoder fills `rlp_node_buf` in nibble order with the
    /// blinded, cached, or newly encoded leaf children for that branch.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, fields(base_path = ?base_path), ret)]
    fn update_cached_rlp(
        arena: &mut NodeArena,
        root: NodeIndex,
        base_path: Nibbles,
        buffers: &mut ArenaTrieBuffers,
        new_epoch: TrieNodeEpoch,
    ) -> RlpNode {
        let cursor = &mut buffers.cursor;
        let rlp_buf = &mut buffers.rlp_buf;
        let rlp_node_buf = &mut buffers.rlp_node_buf;
        let updates = &mut buffers.updates;

        rlp_node_buf.clear();
        // Read before borrowing mutably so hashing an unchanged snapshot keeps its slots shared.
        if let Some(rlp_node) =
            node_state_ref(arena, root).and_then(ArenaSparseNodeState::cached_rlp_node)
        {
            return rlp_node.clone();
        }

        // Step 1: Handle trivial roots that don't need the stack-based walk.
        // Empty roots and leaves are encoded in place. Already-cached branches need no work.
        // Only dirty branches enter the main loop below.
        if has_blinded_branch(arena, root) {
            Self::encode_blinded_extension(arena, root, rlp_buf);
            return node_state_ref(arena, root)
                .and_then(ArenaSparseNodeState::cached_rlp_node)
                .expect("extension was encoded")
                .clone();
        }
        match &arena[root] {
            ArenaSparseNode::EmptyRoot { state } => {
                let node_epoch = match state {
                    ArenaSparseNodeState::Cached { epoch, .. } => *epoch,
                    ArenaSparseNodeState::Revealed => TrieNodeEpoch::UNMODIFIED,
                    ArenaSparseNodeState::Dirty => new_epoch,
                };
                let rlp_node = RlpNode::word_rlp(&EMPTY_ROOT_HASH);
                *arena[root].state_mut() =
                    ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone(), epoch: node_epoch };
                return rlp_node
            }
            ArenaSparseNode::Leaf { .. } => {
                Self::encode_leaf(arena, root, rlp_buf, rlp_node_buf, new_epoch);
                return rlp_node_buf.pop().expect("encode_leaf must push an RlpNode");
            }
            ArenaSparseNode::Branch(_) | ArenaSparseNode::Extension { .. } => {
                if let Some(ArenaSparseNodeState::Cached { rlp_node, .. }) =
                    node_state_ref(arena, root)
                {
                    let rlp_node = rlp_node.clone();
                    return rlp_node;
                }
            }
            ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie | ArenaSparseNode::Free => {
                unreachable!("Subtrie/TakenSubtrie/Free cannot be a subtrie's root");
            }
        }

        cursor.reset(arena, root, base_path);

        // Step 2: Walk dirty branches depth-first using `cursor.next`. Only dirty branches
        // are descended into; all other children (leaves, cached branches, blinded, subtries)
        // are encoded when their parent branch is popped.
        loop {
            let result = cursor.next(&mut *arena, |_, node| {
                matches!(node, ArenaSparseNode::Branch(_) | ArenaSparseNode::Extension { .. }) &&
                    !node.is_cached()
            });

            match result {
                NextResult::Done => break,
                NextResult::NonBranch => {
                    let idx = cursor.head().expect("cursor is non-empty").index;
                    Self::encode_blinded_extension(arena, idx, rlp_buf);
                    continue;
                }
                NextResult::Branch => {}
            };

            let head_idx = cursor.head().expect("cursor is non-empty").index;
            let head_path = cursor.head_path();

            // The branch at `head_idx` is exhausted. All its dirty child branches
            // have already been encoded and cached. Collect all children's RLP nodes
            // and encode the branch.
            trace!(
                target: TRACE_TARGET,
                branch_path = ?head_path,
                branch_short_key = ?arena[head_idx].extension_key(),
                state_mask = ?branch_data(arena, head_idx).state_mask,
                "Calculating branch RlpNode",
            );

            rlp_node_buf.clear();
            // A changed extension key can wrap an unchanged branch whose epoch includes
            // modifications to descendants that have since been removed or pruned.
            let mut node_epoch = branch_data(arena, head_idx)
                .state
                .cached_epoch()
                .unwrap_or(TrieNodeEpoch::UNMODIFIED);
            let state_mask = branch_data(arena, head_idx).state_mask;
            for (dense_idx, _nibble) in BranchChildIter::new(state_mask) {
                let child = branch_data(arena, head_idx).children[dense_idx];
                match child.revealed_index() {
                    None => {
                        rlp_node_buf.push(arena.blinded(child).clone());
                    }
                    Some(child_idx) => {
                        match &arena[child_idx] {
                            ArenaSparseNode::Leaf { .. } => {
                                Self::encode_leaf(
                                    arena,
                                    child_idx,
                                    rlp_buf,
                                    rlp_node_buf,
                                    new_epoch,
                                );
                            }
                            ArenaSparseNode::Branch(child_b) => {
                                let ArenaSparseNodeState::Cached { rlp_node, .. } = &child_b.state
                                else {
                                    panic!("child branch must be cached after DFS");
                                };
                                let rlp_node = rlp_node.clone();
                                rlp_node_buf.push(rlp_node);
                            }
                            ArenaSparseNode::Extension { state, .. } => {
                                let ArenaSparseNodeState::Cached { rlp_node, .. } = state else {
                                    panic!("extension must be cached after DFS");
                                };
                                rlp_node_buf.push(rlp_node.clone());
                            }
                            ArenaSparseNode::Subtrie(subtrie) => {
                                let Some(ArenaSparseNodeState::Cached { rlp_node, .. }) =
                                    node_state_ref(&subtrie.arena, subtrie.root)
                                else {
                                    panic!("subtrie root must be cached");
                                };
                                rlp_node_buf.push(rlp_node.clone());
                            }
                            ArenaSparseNode::TakenSubtrie |
                            ArenaSparseNode::EmptyRoot { .. } |
                            ArenaSparseNode::Free => {
                                unreachable!("Unexpected child {:?}", arena[child_idx]);
                            }
                        }
                        let Some(ArenaSparseNodeState::Cached { epoch: child_epoch, .. }) =
                            node_state_ref(arena, child_idx)
                        else {
                            panic!("revealed child must be cached after encoding");
                        };
                        node_epoch = node_epoch.max(*child_epoch);
                    }
                }
            }

            // Encode the branch, optionally wrapping in its parent extension.
            let b = branch_data(arena, head_idx);
            let short_key = arena[head_idx].extension_key().copied().unwrap_or_default();
            let state_mask = b.state_mask;
            let prev_branch_masks = b.branch_masks;
            let new_branch_masks = Self::get_branch_masks(arena, b);
            let was_dirty = matches!(b.state, ArenaSparseNodeState::Dirty);
            if was_dirty ||
                matches!(node_state_ref(arena, head_idx), Some(ArenaSparseNodeState::Dirty))
            {
                node_epoch = node_epoch.max(new_epoch);
            }

            rlp_buf.clear();
            let rlp_node = BranchNodeRef::new(rlp_node_buf, state_mask).rlp(rlp_buf);

            let branch = branch_data_mut(arena, head_idx);
            branch.state =
                ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone(), epoch: node_epoch };
            branch.branch_masks = new_branch_masks;

            let rlp_node = if short_key.is_empty() {
                rlp_node
            } else {
                rlp_buf.clear();
                ExtensionNodeRef::new(&short_key, &rlp_node).rlp(rlp_buf)
            };

            trace!(
                target: TRACE_TARGET,
                path = ?head_path,
                short_key = ?arena[head_idx].short_key(),
                children = ?state_mask.iter().zip(rlp_node_buf.iter()).collect::<Vec<_>>(),
                rlp_node = ?rlp_node,
                "Calculated branch RlpNode",
            );

            *node_state_mut(arena, head_idx) =
                ArenaSparseNodeState::Cached { rlp_node, epoch: node_epoch };

            // Record trie updates for dirty branches only.
            // Skip the root node (empty logical path) as PST does.
            if let Some(trie_updates) = updates.as_mut().filter(|_| was_dirty) {
                let mut logical_path = head_path;
                logical_path.extend(&short_key);

                if !logical_path.is_empty() {
                    if !prev_branch_masks.is_empty() && new_branch_masks.is_empty() {
                        trie_updates.updated_nodes.remove(&logical_path);
                        trie_updates.removed_nodes.insert(logical_path);
                    } else if !new_branch_masks.is_empty() {
                        let compact = branch_data(arena, head_idx).branch_node_compact(arena);
                        trie_updates.updated_nodes.insert(logical_path, compact);
                        trie_updates.removed_nodes.remove(&logical_path);
                    }
                }
            }
        }

        let Some(ArenaSparseNodeState::Cached { rlp_node, .. }) = node_state_ref(arena, root)
        else {
            panic!("root must be cached after update_cached_rlp");
        };
        rlp_node.clone()
    }

    /// Caches an unmodified extension without decoding its backing branch.
    fn encode_blinded_extension(arena: &mut NodeArena, idx: NodeIndex, rlp_buf: &mut Vec<u8>) {
        if arena[idx].is_cached() {
            return;
        }
        let ArenaSparseNode::Extension { key, child: ExtensionChild::Blinded(rlp_node), state } =
            &mut arena[idx]
        else {
            unreachable!("expected extension with blinded branch")
        };
        rlp_buf.clear();
        let rlp_node = ExtensionNodeRef::new(key, rlp_node).rlp(rlp_buf);
        *state = ArenaSparseNodeState::Cached { rlp_node, epoch: TrieNodeEpoch::UNMODIFIED };
    }

    /// Immutable traversal to find a leaf value at `full_path` starting from `root` in `arena`.
    /// `path_offset` is the number of nibbles already consumed from `full_path`.
    fn get_leaf_value_in_arena<'a>(
        arena: &'a NodeArena,
        mut current: NodeIndex,
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
                ArenaSparseNode::Extension { key, child, .. } => {
                    let logical_end = path_offset + key.len();
                    if full_path.len() <= logical_end ||
                        full_path.slice_unchecked(path_offset, logical_end) != *key
                    {
                        return None;
                    }
                    current = child.revealed_index()?;
                    path_offset = logical_end;
                }
                ArenaSparseNode::Branch(b) => {
                    if full_path.len() <= path_offset {
                        return None;
                    }

                    let child_nibble = full_path.get_unchecked(path_offset);
                    let child_idx = BranchChildIdx::new(b.state_mask, child_nibble)?;
                    current = b.children[child_idx].revealed_index()?;
                    path_offset += 1;
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
        mut current: NodeIndex,
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
                ArenaSparseNode::Extension { key, child, .. } => {
                    let logical_end = path_offset + key.len();
                    if full_path.len() <= logical_end ||
                        full_path.slice(path_offset..logical_end) != *key
                    {
                        return Ok(LeafLookup::NonExistent);
                    }
                    match child {
                        ExtensionChild::Revealed(index) => current = *index,
                        ExtensionChild::Blinded(rlp_node) => {
                            return Err(LeafLookupError::BlindedNode {
                                path: full_path.slice(..logical_end),
                                hash: rlp_node
                                    .as_hash()
                                    .unwrap_or_else(|| keccak256(rlp_node.as_slice())),
                            })
                        }
                    }
                    path_offset = logical_end;
                }
                ArenaSparseNode::Branch(b) => {
                    if full_path.len() <= path_offset {
                        return Ok(LeafLookup::NonExistent);
                    }

                    let child_nibble = full_path.get_unchecked(path_offset);
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
                            let mut blinded_path = full_path.slice(..path_offset);
                            blinded_path.push_unchecked(child_nibble);
                            return Err(LeafLookupError::BlindedNode { path: blinded_path, hash });
                        }
                        Some(child_idx) => {
                            current = child_idx;
                            path_offset += 1;
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

    /// Encodes an uncached leaf and stores its RLP and epoch.
    fn encode_leaf(
        arena: &mut NodeArena,
        idx: NodeIndex,
        rlp_buf: &mut Vec<u8>,
        rlp_node_buf: &mut Vec<RlpNode>,
        new_epoch: TrieNodeEpoch,
    ) {
        let (key, value, state) = match &arena[idx] {
            ArenaSparseNode::Leaf { key, value, state } => (key, value, state),
            _ => unreachable!("encode_leaf called on non-Leaf node"),
        };

        let epoch = match state {
            ArenaSparseNodeState::Cached { rlp_node, .. } => {
                rlp_node_buf.push(rlp_node.clone());
                return;
            }
            ArenaSparseNodeState::Revealed => TrieNodeEpoch::UNMODIFIED,
            ArenaSparseNodeState::Dirty => new_epoch,
        };

        rlp_buf.clear();
        let rlp_node = LeafNodeRef { key, value }.rlp(rlp_buf);

        *arena[idx].state_mut() =
            ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone(), epoch };
        rlp_node_buf.push(rlp_node);
    }

    /// Counts the total leaves and dirty leaves in a subtree rooted at `idx`.
    fn count_leaves_and_dirty(arena: &NodeArena, idx: NodeIndex) -> (u64, u64) {
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
            ArenaSparseNode::Extension { child, .. } => child
                .revealed_index()
                .map_or((0, 0), |child| Self::count_leaves_and_dirty(arena, child)),
            _ => (0, 0),
        }
    }

    /// Asserts that every node in the upper arena satisfies the subtrie structure invariant:
    /// - Nodes at `UPPER_TRIE_MAX_DEPTH` path length must be `Subtrie` (or `TakenSubtrie`).
    /// - Nodes at other depths must NOT be `Subtrie`.
    ///
    /// Uses the cursor to DFS the upper arena, checking each visited node's path length.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    #[cfg(all(debug_assertions, not(feature = "fuzz")))]
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
        src_idx: NodeIndex,
        dst_slot: Option<NodeIndex>,
    ) -> NodeIndex {
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

    /// Moves a node's children from `src` to `dst`: revealed children are migrated recursively
    /// with [`Self::migrate_nodes`], blinded children have their RLP moved into `dst`'s side
    /// table. `node` must already be detached from `src`.
    fn migrate_children(dst: &mut NodeArena, src: &mut NodeArena, node: &mut ArenaSparseNode) {
        match node {
            ArenaSparseNode::Extension { child: ExtensionChild::Revealed(child), .. } => {
                *child = Self::migrate_nodes(dst, src, *child, None);
            }
            ArenaSparseNode::Branch(branch) => {
                for child in &mut branch.children {
                    *child = match child.revealed_index() {
                        Some(child_idx) => {
                            BranchChild::revealed(Self::migrate_nodes(dst, src, child_idx, None))
                        }
                        None => dst.insert_blinded(src.take_blinded(*child)),
                    };
                }
            }
            _ => {}
        }
    }

    /// Reveals a single proof node using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// If the result is `Blinded`, the blinded child is replaced with the proof node (converted to
    /// an arena node with `Cached` state). All other cases (already revealed, no child, diverged,
    /// leaf head) are no-ops — the proof node is skipped.
    ///
    /// Returns the `NodeIndex` of the revealed node in the arena, if any was revealed.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn reveal_node(
        arena: &mut NodeArena,
        cursor: &ArenaCursor,
        node: &mut ProofTrieNode,
        extension_child: Option<ProofTrieNode>,
        find_result: SeekResult,
    ) -> Option<NodeIndex> {
        let SeekResult::Blinded = find_result else {
            // Already revealed, no child slot, or diverged — skip this proof node.
            return None;
        };

        let head = cursor.head().expect("cursor is non-empty");
        let head_idx = head.index;
        if let ArenaSparseNode::Extension { child: ExtensionChild::Blinded(rlp_node), .. } =
            &arena[head_idx]
        {
            if node.path != cursor.head_logical_branch_path(arena) ||
                !matches!(node.node, TrieNode::Branch(_))
            {
                return None;
            }
            let state = ArenaSparseNodeState::Cached {
                rlp_node: rlp_node.clone(),
                epoch: TrieNodeEpoch::UNMODIFIED,
            };
            let proof = mem::replace(node, ProofTrieNode::empty());
            let mut branch = ArenaSparseNode::from_proof_node(arena, proof, None);
            *branch.state_mut() = state;
            let child_idx = arena.insert(branch);
            let ArenaSparseNode::Extension { child, .. } = &mut arena[head_idx] else {
                unreachable!()
            };
            *child = ExtensionChild::Revealed(child_idx);
            // Hydration does not change the extension's cached RLP or epoch.
            return None;
        }
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
        let head_branch = branch_data(arena, head_idx);
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

        let proof_node = mem::replace(node, ProofTrieNode::empty());
        let node = ArenaSparseNode::from_proof_node(arena, proof_node, extension_child);
        let child_idx = arena.insert(node);
        *node_state_mut(arena, child_idx) =
            ArenaSparseNodeState::Cached { rlp_node: cached_rlp, epoch: TrieNodeEpoch::UNMODIFIED };
        branch_data_mut(arena, head_idx).children[dense_child_idx] =
            BranchChild::revealed(child_idx);
        arena.take_blinded(child);

        Some(child_idx)
    }

    /// Converts a calculator proof node, retaining its opaque backing branch if not supplied.
    fn node_from_proof(
        arena: &mut NodeArena,
        proof_node: ProofTrieNode,
        extension_child: Option<ProofTrieNode>,
    ) -> ArenaSparseNode {
        ArenaSparseNode::from_proof_node(arena, proof_node, extension_child)
    }

    #[cfg(all(debug_assertions, not(feature = "fuzz")))]
    fn collect_reachable_nodes(
        arena: &NodeArena,
        idx: NodeIndex,
        reachable: &mut alloy_primitives::map::HashSet<NodeIndex>,
    ) {
        if !reachable.insert(idx) {
            return;
        }
        match &arena[idx] {
            ArenaSparseNode::Extension { child: ExtensionChild::Revealed(child), .. } => {
                Self::collect_reachable_nodes(arena, *child, reachable);
            }
            ArenaSparseNode::Branch(b) => {
                for child in &b.children {
                    if let Some(child_idx) = child.revealed_index() {
                        Self::collect_reachable_nodes(arena, child_idx, reachable);
                    }
                }
            }
            _ => {}
        }
    }

    #[cfg(all(debug_assertions, not(feature = "fuzz")))]
    fn assert_no_orphaned_nodes(arena: &NodeArena, root: NodeIndex, label: &str) {
        let mut reachable = alloy_primitives::map::HashSet::default();
        Self::collect_reachable_nodes(arena, root, &mut reachable);
        let all_indices: alloy_primitives::map::HashSet<NodeIndex> =
            arena.iter().map(|(idx, _)| idx).collect();
        let orphaned: Vec<_> = all_indices.difference(&reachable).collect();
        debug_assert!(
            orphaned.is_empty(),
            "{label} has {} orphaned node(s): {orphaned:?}",
            orphaned.len(),
        );
    }
}

#[cfg(all(debug_assertions, not(feature = "fuzz")))]
impl Drop for PersistentArenaParallelSparseTrie {
    fn drop(&mut self) {
        // An interrupted mutation may leave the arena inconsistent. Keep the original panic.
        if std::thread::panicking() {
            return;
        }
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

impl Clone for PersistentArenaParallelSparseTrie {
    fn clone(&self) -> Self {
        let upper_arena = self.upper_arena.deep_clone(|node| match node {
            ArenaSparseNode::Subtrie(subtrie) => {
                ArenaSparseNode::Subtrie(Box::new(subtrie.clone_independent()))
            }
            node => node.clone(),
        });
        Self {
            upper_arena,
            root: self.root,
            buffers: self.buffers.clone(),
            parallelism_thresholds: self.parallelism_thresholds,
        }
    }
}

impl Default for PersistentArenaParallelSparseTrie {
    fn default() -> Self {
        let mut upper_arena = NodeArena::new();
        let root = upper_arena
            .insert(ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Revealed });
        Self {
            upper_arena,
            root,
            buffers: ArenaTrieBuffers::default(),
            parallelism_thresholds: PersistentArenaParallelismThresholds::default(),
        }
    }
}

impl PersistentArenaParallelSparseTrie {
    /// Caches a subtrie root if needed and merges its buffered trie updates.
    fn update_upper_subtrie(&mut self, head_idx: NodeIndex, new_epoch: TrieNodeEpoch) {
        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx] else {
            unreachable!()
        };

        if !node_state_ref(&subtrie.arena, subtrie.root)
            .is_some_and(|state| matches!(state, ArenaSparseNodeState::Cached { .. }))
        {
            subtrie.update_cached_rlp(new_epoch);
        }

        Self::merge_subtrie_updates(&mut self.buffers.updates, &mut subtrie.buffers.updates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LeafUpdate, SparseTrie};
    use alloy_primitives::{map::B256Map, B256, U256};
    use alloy_trie::TrieMask;
    use rand::{seq::SliceRandom, Rng, SeedableRng};
    use reth_execution_errors::SparseTrieErrorKind;
    use reth_trie::{test_utils::TrieTestHarness, HashBuilder};
    use reth_trie_common::{LeafNode, ProofV2Target};
    use std::collections::BTreeMap;
    use tracing::{info, trace};

    const fn epoch(value: u64) -> TrieNodeEpoch {
        TrieNodeEpoch::new(value)
    }

    #[test]
    fn invalid_extension_child_preserves_retention_and_allows_valid_root() {
        let mut trie = PersistentArenaParallelSparseTrie::default();
        let root = TrieNode::Extension(reth_trie_common::ExtensionNode::new(
            Nibbles::from_nibbles([1]),
            RlpNode::word_rlp(&B256::ZERO),
        ));
        let error = trie
            .set_root(
                &mut [
                    ProofTrieNode { path: Nibbles::new(), node: root, masks: None },
                    ProofTrieNode {
                        path: Nibbles::from_nibbles([1]),
                        node: TrieNode::Leaf(LeafNode::new(Nibbles::from_nibbles([2]), vec![1])),
                        masks: None,
                    },
                ],
                true,
            )
            .unwrap_err();
        assert!(
            matches!(error.kind(), SparseTrieErrorKind::Reveal { path, .. } if path.is_empty())
        );
        assert!(trie.buffers.updates.is_none());

        trie.set_root(&mut [ProofTrieNode::empty()], true).unwrap();
        assert_eq!(trie.root(epoch(1)), EMPTY_ROOT_HASH);
        assert!(trie.buffers.updates.is_some());
    }

    #[test]
    fn pruning_pops_cursor_before_reusing_node_slot() {
        use super::{
            ArenaCursor, ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeState, BranchChild,
            NextResult, NodeArena,
        };
        use reth_trie_common::RlpNode;

        let mut arena = NodeArena::new();
        let rlp = RlpNode::word_rlp(&B256::repeat_byte(1));
        let state = ArenaSparseNodeState::Cached { rlp_node: rlp.clone(), epoch: epoch(0) };
        let leaf =
            ArenaSparseNode::Leaf { state: state.clone(), key: Default::default(), value: vec![1] };
        let first = arena.insert(leaf.clone());
        let second = arena.insert(leaf);
        let root = arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
            state: state.clone(),
            children: [BranchChild::revealed(first), BranchChild::revealed(second)]
                .into_iter()
                .collect(),
            state_mask: alloy_trie::TrieMask::new(3),
            branch_masks: Default::default(),
        }));
        let mut cursor = ArenaCursor::default();
        cursor.reset(&arena, root, Default::default());
        assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::NonBranch));
        assert_eq!(cursor.head().unwrap().index, first);

        PersistentArenaParallelSparseTrie::remove_pruned_node(&mut arena, &mut cursor);
        assert_eq!(cursor.head().unwrap().index, root);
        let reused = arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Dirty,
            key: Default::default(),
            value: vec![2],
        });
        assert_eq!(reused, first);

        assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::NonBranch));
        assert_eq!(cursor.head().unwrap().index, second);
        assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::Branch));
        assert_eq!(arena[root].state_ref(), Some(&state));
        assert_eq!(arena.blinded(arena[root].branch_ref().children[0]), &rlp);
        assert!(matches!(cursor.next(&mut arena, |_, _| true), NextResult::Done));
    }

    /// Test harness for proptest-based arena sparse trie testing.
    ///
    /// Wraps [`TrieTestHarness`] and adds `PersistentArenaParallelSparseTrie`-specific helpers for
    /// the reveal-update loop and asserting that sparse trie updates match `StorageRoot`.
    struct ArenaTrieTestHarness {
        /// The inner general-purpose harness.
        inner: TrieTestHarness,
    }

    impl std::ops::Deref for ArenaTrieTestHarness {
        type Target = TrieTestHarness;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl std::ops::DerefMut for ArenaTrieTestHarness {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }

    impl ArenaTrieTestHarness {
        /// Creates a new test harness from a map of hashed storage slots to values.
        fn new(storage: BTreeMap<B256, U256>) -> Self {
            Self { inner: TrieTestHarness::new(storage) }
        }

        /// Computes the new storage root and trie updates after applying the given changes
        /// using both `StorageRoot` and the provided `PersistentArenaParallelSparseTrie`, then
        /// asserts they match.
        fn assert_changes(
            &self,
            apst: &mut PersistentArenaParallelSparseTrie,
            changes: BTreeMap<B256, U256>,
        ) {
            // Compute expected root and trie updates via StorageRoot.
            let (expected_root, mut expected_trie_updates) = if changes.is_empty() {
                (self.original_root(), Default::default())
            } else {
                self.get_root_with_updates(&changes)
            };

            self.minimize_trie_updates(&mut expected_trie_updates);

            // Build leaf updates for the APST: non-zero values are upserts (RLP-encoded),
            // zero values are deletions (empty vec).
            let mut leaf_updates: B256Map<LeafUpdate> = changes
                .iter()
                .map(|(&slot, &value)| {
                    let rlp_value = if value.is_zero() {
                        Vec::new()
                    } else {
                        alloy_rlp::encode_fixed_size(&value).to_vec()
                    };
                    (slot, LeafUpdate::Changed(rlp_value))
                })
                .collect();

            // Reveal-update loop: call update_leaves, collect required proofs, fetch them,
            // reveal, and repeat until no more proofs are needed.
            loop {
                let mut targets: Vec<ProofV2Target> = Vec::new();
                apst.update_leaves(&mut leaf_updates, |key, parent| {
                    targets.push(ProofV2Target::new(key).with_parent(parent));
                })
                .expect("update_leaves should succeed");

                if targets.is_empty() {
                    break;
                }

                let (mut proof_nodes, _) = self.proof_v2(&mut targets);
                apst.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");
            }

            // Compute root and take updates from the APST.
            let actual_root = apst.root(epoch(0));
            let mut actual_updates = apst.take_updates();

            // Minimize sparse updates inline (can't use TrieTestHarness::minimize_sparse_updates
            // due to the crate's SparseTrieUpdates being a different type than reth-trie's copy).
            actual_updates.updated_nodes.retain(|path, node| {
                self.storage_trie_updates().storage_nodes.get(path) != Some(node)
            });
            actual_updates
                .removed_nodes
                .retain(|path| self.storage_trie_updates().storage_nodes.contains_key(path));

            let mut expected_updated_nodes =
                expected_trie_updates.storage_nodes.into_iter().collect::<Vec<_>>();
            let mut actual_updated_nodes =
                actual_updates.updated_nodes.into_iter().collect::<Vec<_>>();
            expected_updated_nodes.sort();
            actual_updated_nodes.sort();
            pretty_assertions::assert_eq!(
                expected_updated_nodes,
                actual_updated_nodes,
                "updated nodes mismatch"
            );

            let mut expected_removed_nodes =
                expected_trie_updates.removed_nodes.into_iter().collect::<Vec<_>>();
            let mut actual_removed_nodes =
                actual_updates.removed_nodes.into_iter().collect::<Vec<_>>();
            expected_removed_nodes.sort();
            actual_removed_nodes.sort();
            pretty_assertions::assert_eq!(
                expected_removed_nodes,
                actual_removed_nodes,
                "removed nodes mismatch"
            );
            assert_eq!(expected_root, actual_root, "storage root mismatch");
        }
    }

    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb;

    /// Builds a changeset by mixing `new_keys` (fresh insertions) with a fraction of
    /// existing keys from `base` (updates/deletions).
    ///
    /// `overlap_pct` controls how many existing keys are included, and `delete_pct`
    /// controls how many of those become deletions (zero values). The remaining
    /// overlap keys get random non-zero values.
    fn build_changeset(
        base: &BTreeMap<B256, U256>,
        new_keys: BTreeMap<B256, U256>,
        overlap_pct: f64,
        delete_pct: f64,
        rng: &mut rand::rngs::StdRng,
    ) -> BTreeMap<B256, U256> {
        let num_overlap = (base.len() as f64 * overlap_pct) as usize;
        let num_delete = (num_overlap as f64 * delete_pct) as usize;

        let mut all_keys: Vec<B256> = base.keys().copied().collect();
        all_keys.shuffle(rng);
        let overlap_keys = &all_keys[..num_overlap];

        let mut changeset = new_keys;
        for (i, &key) in overlap_keys.iter().enumerate() {
            let value =
                if i < num_delete { U256::ZERO } else { U256::from(rng.random::<u64>() | 1) };
            changeset.entry(key).or_insert(value);
        }
        changeset
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]
        #[test]
        fn arena_trie_proptest(
            initial in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=100usize),
            changeset1_new_keys in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=30usize),
            changeset2_new_keys in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=30usize),
            overlap_pct in 0.0..=0.5f64,
            delete_pct in 0.0..=0.33f64, // percent of overlapping changeset which are deletes
            shuffle_seed in arb::<u64>(),
        ) {
            reth_tracing::init_test_tracing();
            info!(target: TRACE_TARGET, ?shuffle_seed, "PROPTEST START");

            // Filter out zero-valued entries from the initial dataset (zeros mean "absent").
            let initial: BTreeMap<B256, U256> = initial.into_iter()
                .filter(|(_, v)| !v.is_zero())
                .collect();

            let mut rng = rand::rngs::StdRng::seed_from_u64(shuffle_seed);

            let changeset1 = build_changeset(&initial, changeset1_new_keys, overlap_pct, delete_pct, &mut rng);
            for (i, (k, v)) in changeset1.iter().enumerate() {
                trace!(target: TRACE_TARGET, ?i, ?k, ?v, "Changeset 1 entry");
            }

            let mut harness = ArenaTrieTestHarness::new(initial);

            // Initialize the APST from the harness root node.
            let mut root_nodes = harness.root_nodes();
            let mut apst = PersistentArenaParallelSparseTrie::default().with_parallelism_thresholds(
                PersistentArenaParallelismThresholds {
                    min_dirty_leaves: 3,
                    min_revealed_nodes: 3,
                    min_updates: 3,
                    min_leaves_for_prune: 3,
                },
            );
            apst.set_root(&mut root_nodes, true).expect("set_root should succeed");

            harness.assert_changes(&mut apst, changeset1.clone());

            // Update the harness base dataset to reflect the first changeset.
            harness.apply_changeset(changeset1);

            // All nodes were cached at epoch 0, so this maximally prunes the trie before the
            // second update round.
            apst.prune(epoch(1));

            let changeset2 = build_changeset(harness.storage(), changeset2_new_keys, overlap_pct, delete_pct, &mut rng);
            for (i, (k, v)) in changeset2.iter().enumerate() {
                trace!(target: TRACE_TARGET, ?i, ?k, ?v, "Changeset 2 entry");
            }

            harness.assert_changes(&mut apst, changeset2);
        }
    }

    const EDITED_KEY: B256 = key_with_prefix(0x11, 0x00);
    const SHARED_PREFIX_KEY: B256 = key_with_prefix(0x11, 0x10);
    const ADJACENT_PREFIX_KEY: B256 = key_with_prefix(0x12, 0x00);
    const DISTANT_BRANCH_KEY: B256 = key_with_prefix(0x20, 0x00);

    const fn key_with_prefix(first_byte: u8, second_byte: u8) -> B256 {
        let mut bytes = [0; 32];
        bytes[0] = first_byte;
        bytes[1] = second_byte;
        B256::new(bytes)
    }

    fn apply_leaf_edits(trie: &mut PersistentArenaParallelSparseTrie, updates: &[(B256, &[u8])]) {
        let mut changes = updates
            .iter()
            .map(|(key, value)| (*key, LeafUpdate::Changed(value.to_vec())))
            .collect();
        trie.update_leaves(&mut changes, |key, parent| {
            panic!("unexpected proof: {key} {parent:?}")
        })
        .unwrap();
        assert!(changes.is_empty());
    }

    fn trie_with_shared_prefix_leaves() -> PersistentArenaParallelSparseTrie {
        let mut trie = PersistentArenaParallelSparseTrie::default().with_parallelism_thresholds(
            PersistentArenaParallelismThresholds {
                min_revealed_nodes: 0,
                min_updates: 0,
                min_dirty_leaves: 0,
                min_leaves_for_prune: 0,
            },
        );
        trie.set_root(&mut [ProofTrieNode::empty()], true).unwrap();
        apply_leaf_edits(
            &mut trie,
            &[
                (EDITED_KEY, &[1; 40]),
                (SHARED_PREFIX_KEY, &[2; 40]),
                (ADJACENT_PREFIX_KEY, &[3; 40]),
                (DISTANT_BRANCH_KEY, &[4; 40]),
            ],
        );
        trie
    }

    fn leaf_value(trie: &PersistentArenaParallelSparseTrie, key: B256) -> &Vec<u8> {
        trie.get_leaf_value(&Nibbles::unpack(key)).unwrap()
    }

    #[test]
    fn snapshot_update_tracking_can_be_toggled_with_revealed_subtries() {
        let mut original = trie_with_shared_prefix_leaves();
        original.root(epoch(1));
        assert!(!original.take_updates().updated_nodes.is_empty());
        let mut snapshot = original.snapshot();

        snapshot.set_updates(false);
        apply_leaf_edits(&mut snapshot, &[(EDITED_KEY, &[5; 40])]);
        snapshot.root(epoch(2));
        assert_eq!(snapshot.take_updates(), SparseTrieUpdates::default());

        snapshot.set_updates(true);
        for trie in [&mut original, &mut snapshot] {
            apply_leaf_edits(trie, &[(EDITED_KEY, &[6; 40])]);
            trie.root(epoch(3));
            assert!(!trie.take_updates().updated_nodes.is_empty());
        }
        assert_eq!(original.root(epoch(3)), snapshot.root(epoch(3)));
    }

    #[test]
    fn snapshots_share_untouched_leaves_and_both_trie_values_can_be_updated() {
        let mut original = trie_with_shared_prefix_leaves();
        let epoch = TrieNodeEpoch::new(1);
        let root = original.root(epoch);
        let mut expected = original.clone();

        let mut snapshot = original.snapshot();
        assert_eq!(snapshot.root_epoch(), Some(epoch));
        assert_eq!(original.updates_ref(), snapshot.updates_ref());
        assert!(!snapshot.take_updates().updated_nodes.is_empty());
        assert!(!original.updates_ref().updated_nodes.is_empty());

        let edits: &[(B256, &[u8])] = &[(EDITED_KEY, &[5; 40]), (DISTANT_BRANCH_KEY, &[6; 40])];
        apply_leaf_edits(&mut snapshot, edits);
        apply_leaf_edits(&mut expected, edits);
        let epoch = TrieNodeEpoch::new(2);
        assert_eq!(snapshot.root(epoch), expected.root(epoch));
        assert_eq!(leaf_value(&original, EDITED_KEY), &[1; 40]);
        assert_eq!(leaf_value(&snapshot, EDITED_KEY), &[5; 40]);
        // Equality alone is insufficient: untouched lower and adjacent leaves share storage.
        assert!(core::ptr::eq(
            leaf_value(&original, SHARED_PREFIX_KEY),
            leaf_value(&snapshot, SHARED_PREFIX_KEY)
        ));
        assert!(core::ptr::eq(
            leaf_value(&original, ADJACENT_PREFIX_KEY),
            leaf_value(&snapshot, ADJACENT_PREFIX_KEY)
        ));
        assert_eq!(original.root(epoch), root);

        // The next snapshot retains pre-existing edits even after structural changes to its peer.
        let mut next = snapshot.snapshot();
        apply_leaf_edits(
            &mut snapshot,
            &[
                (EDITED_KEY, &[7; 40]),
                (SHARED_PREFIX_KEY, &[]),
                (key_with_prefix(0x11, 0x20), &[9; 40]),
            ],
        );
        apply_leaf_edits(&mut original, &[(ADJACENT_PREFIX_KEY, &[8; 40])]);
        assert_eq!(leaf_value(&next, EDITED_KEY), &[5; 40]);
        assert_eq!(leaf_value(&next, SHARED_PREFIX_KEY), &[2; 40]);
        assert_eq!(leaf_value(&next, ADJACENT_PREFIX_KEY), &[3; 40]);
        assert_eq!(next.root(epoch), expected.root(epoch));

        drop(snapshot);
        drop(original);
        apply_leaf_edits(&mut next, &[(EDITED_KEY, &[10; 40]), (DISTANT_BRANCH_KEY, &[])]);
        apply_leaf_edits(&mut expected, &[(EDITED_KEY, &[10; 40]), (DISTANT_BRANCH_KEY, &[])]);
        let epoch = TrieNodeEpoch::new(3);
        assert_eq!(next.root(epoch), expected.root(epoch));
    }

    #[test]
    fn pruning_a_snapshot_keeps_other_trie_values_revealed() {
        let mut original = trie_with_shared_prefix_leaves();
        original.root(TrieNodeEpoch::new(1));
        let mut expected = original.clone();
        let mut snapshot = original.snapshot();

        for trie in [&mut snapshot, &mut expected] {
            apply_leaf_edits(trie, &[(EDITED_KEY, &[5; 40])]);
        }
        let epoch = TrieNodeEpoch::new(2);
        let snapshot_root = snapshot.root(epoch);
        assert_eq!(snapshot_root, expected.root(epoch));
        assert_ne!(snapshot_root, original.root(epoch));
        // The peer is still live, so pruning must keep the retained slot in place.
        let before = leaf_value(&snapshot, EDITED_KEY).as_ptr();
        let pruned = snapshot.prune(epoch);
        assert!(pruned > 0);
        assert_eq!(pruned, expected.prune(epoch));
        assert_eq!(snapshot.prune(epoch), 0);
        assert_eq!(before, leaf_value(&snapshot, EDITED_KEY).as_ptr());
        assert_eq!(snapshot.root(epoch), snapshot_root);
        assert_eq!(leaf_value(&original, SHARED_PREFIX_KEY), &[2; 40]);
        assert_eq!(leaf_value(&original, ADJACENT_PREFIX_KEY), &[3; 40]);
        assert_eq!(leaf_value(&original, DISTANT_BRANCH_KEY), &[4; 40]);

        let absent = Nibbles::unpack(SHARED_PREFIX_KEY);
        assert_eq!(snapshot.find_leaf(&absent, None), expected.find_leaf(&absent, None));
        assert!(matches!(
            snapshot.find_leaf(&absent, None),
            Err(LeafLookupError::BlindedNode { .. })
        ));

        let mut proof = [ProofTrieNode {
            path: absent.slice(..3),
            node: TrieNode::Leaf(LeafNode::new(absent.slice(3..), vec![2; 40])),
            masks: None,
        }];
        snapshot.reveal_nodes(&mut proof).unwrap();
        assert_eq!(leaf_value(&snapshot, SHARED_PREFIX_KEY), &[2; 40]);
        assert_eq!(snapshot.root(epoch), snapshot_root);

        snapshot.clear();
        assert_eq!(snapshot.root(epoch), EMPTY_ROOT_HASH);
        assert_eq!(leaf_value(&original, EDITED_KEY), &[1; 40]);
    }

    #[test]
    fn pruning_reclaims_exclusive_snapshot_storage_without_new_evictions() {
        let prune_epoch = epoch(2);
        for cleanup_epoch in [prune_epoch, TrieNodeEpoch::UNMODIFIED] {
            let mut original = trie_with_shared_prefix_leaves();
            let root = original.root(epoch(1));
            let mut fork = original.snapshot();

            assert!(fork.prune(prune_epoch) > 0);
            assert_eq!(fork.upper_arena.len(), 1);
            assert!(fork.upper_arena.has_vacant_slots());
            assert_eq!(fork.prune(cleanup_epoch), 0);
            assert!(fork.upper_arena.is_snapshot_ready());
            assert_eq!(leaf_value(&original, EDITED_KEY), &[1; 40]);
            assert_eq!(original.root(prune_epoch), root);

            drop(original);
            assert!(fork.upper_arena.is_snapshot_ready());
            assert_eq!(fork.prune(cleanup_epoch), 0);
            assert!(!fork.upper_arena.is_snapshot_ready());
            assert!(!fork.upper_arena.has_vacant_slots());
            assert_eq!(fork.upper_arena.len(), 1);
            assert!(matches!(
                fork.find_leaf(&Nibbles::unpack(EDITED_KEY), None),
                Err(LeafLookupError::BlindedNode { .. })
            ));
            assert_eq!(fork.root(prune_epoch), root);
        }
    }

    #[test]
    fn applying_a_prune_plan_to_an_exclusive_subtrie_compacts_live_nodes() {
        let mut trie = trie_with_shared_prefix_leaves();
        let adjacent = key_with_prefix(0x11, 0x01);
        apply_leaf_edits(&mut trie, &[(adjacent, &[5; 40])]);
        trie.root(epoch(1));
        apply_leaf_edits(&mut trie, &[(SHARED_PREFIX_KEY, &[6; 40])]);
        let cutoff = epoch(2);
        let root = trie.root(cutoff);

        let path = Nibbles::from_nibbles([1, 1]);
        let (subtrie_idx, plan) = trie
            .upper_arena
            .iter()
            .find_map(|(idx, node)| match node {
                ArenaSparseNode::Subtrie(subtrie) if subtrie.path == path => {
                    subtrie.plan_prune(cutoff).map(|plan| (idx, plan))
                }
                _ => None,
            })
            .unwrap();
        let ArenaSparseNode::Subtrie(subtrie) = &mut trie.upper_arena[subtrie_idx] else {
            unreachable!()
        };
        assert!(subtrie.arena.can_compact());
        assert_eq!(subtrie.apply_prune_plan(plan), 3);
        assert_eq!(subtrie.num_leaves, 1);
        assert!(subtrie.arena.iter().map(|(idx, _)| idx.get()).eq(0..subtrie.arena.len()));
        assert_eq!(trie.root(cutoff), root);
        assert_eq!(leaf_value(&trie, SHARED_PREFIX_KEY), &[6; 40]);
    }

    #[test]
    fn pruning_compacts_exclusive_subtries_with_a_live_snapshot() {
        for min_leaves_for_prune in [u64::MAX, 0] {
            let mut original = trie_with_shared_prefix_leaves();
            let cutoff = epoch(1);
            let original_root = original.root(cutoff);
            let mut fork = original.snapshot();
            fork.parallelism_thresholds.min_leaves_for_prune = min_leaves_for_prune;

            // Both lower branches are created after the snapshot. Deleting the middle child
            // leaves a hole without collapsing either branch.
            let mut leaves: BTreeMap<B256, &[u8]> = BTreeMap::from([
                (EDITED_KEY, &[1; 40][..]),
                (SHARED_PREFIX_KEY, &[2; 40][..]),
                (ADJACENT_PREFIX_KEY, &[3; 40][..]),
                (DISTANT_BRANCH_KEY, &[4; 40][..]),
            ]);
            for prefix in [0x13, 0x14] {
                let first = key_with_prefix(prefix, 0x00);
                let middle = key_with_prefix(prefix, 0x10);
                let last = key_with_prefix(prefix, 0x20);
                apply_leaf_edits(
                    &mut fork,
                    &[(first, &[5; 40]), (middle, &[6; 40]), (last, &[7; 40])],
                );
                apply_leaf_edits(&mut fork, &[(middle, &[])]);
                leaves.insert(first, &[5; 40]);
                leaves.insert(last, &[7; 40]);
            }
            assert_matches_leaves(&mut fork, &leaves, cutoff);
            let fork_root = fork.root(cutoff);
            assert_ne!(fork_root, original_root);

            let paths = [Nibbles::from_nibbles([1, 3]), Nibbles::from_nibbles([1, 4])];
            let exclusive_subtries = |trie: &PersistentArenaParallelSparseTrie| {
                trie.upper_arena
                    .iter()
                    .filter_map(|(_, node)| match node {
                        ArenaSparseNode::Subtrie(subtrie) if paths.contains(&subtrie.path) => {
                            Some(subtrie)
                        }
                        _ => None,
                    })
                    .map(|subtrie| {
                        assert!(subtrie.arena.can_compact());
                        let dense = subtrie
                            .arena
                            .iter()
                            .map(|(idx, _)| idx.get())
                            .eq(0..subtrie.arena.len());
                        (subtrie.root, dense)
                    })
                    .collect::<Vec<_>>()
            };
            let before = exclusive_subtries(&fork);
            assert_eq!(before.len(), 2);
            assert!(before.iter().all(|(_, dense)| !dense));
            assert!(exclusive_subtries(&original).is_empty());
            assert!(fork.upper_arena.nodes_mutation_copies());

            assert_eq!(fork.prune(TrieNodeEpoch::UNMODIFIED), 0);
            assert_eq!(exclusive_subtries(&fork), before);

            // Every node was hashed at the cutoff, so this is compaction without eviction.
            assert_eq!(fork.prune(cutoff), 0);
            assert_eq!(exclusive_subtries(&fork), vec![(NodeIndex::new(0), true); 2]);
            assert_eq!(fork.root(cutoff), fork_root);
            for (key, value) in leaves {
                assert_eq!(leaf_value(&fork, key), value);
            }
            assert_eq!(original.root(cutoff), original_root);
            assert_eq!(leaf_value(&original, EDITED_KEY), &[1; 40]);
        }
    }

    #[test]
    fn pruning_compacts_leaf_subtries_after_a_snapshot_drops() {
        for min_leaves_for_prune in [u64::MAX, 0] {
            let mut original = trie_with_shared_prefix_leaves();
            let cutoff = epoch(1);
            let original_root = original.root(cutoff);
            let mut fork = original.snapshot();
            fork.parallelism_thresholds.min_leaves_for_prune = min_leaves_for_prune;
            apply_leaf_edits(&mut fork, &[(SHARED_PREFIX_KEY, &[])]);
            let root = fork.root(epoch(2));

            let path = Nibbles::from_nibbles([1, 1]);
            let subtrie_index = |trie: &PersistentArenaParallelSparseTrie| {
                trie.upper_arena
                    .iter()
                    .find_map(|(idx, node)| match node {
                        ArenaSparseNode::Subtrie(subtrie) if subtrie.path == path => Some(idx),
                        _ => None,
                    })
                    .unwrap()
            };
            let subtrie_idx = subtrie_index(&fork);
            let ArenaSparseNode::Subtrie(subtrie) = &mut fork.upper_arena[subtrie_idx] else {
                unreachable!()
            };
            assert!(matches!(subtrie.arena[subtrie.root], ArenaSparseNode::Leaf { .. }));
            assert_eq!(subtrie.arena.len(), 1);
            assert!(!subtrie.arena.can_compact());
            assert_eq!(subtrie.prune(cutoff), 0);
            assert_eq!(original.root(cutoff), original_root);
            assert_eq!(leaf_value(&original, SHARED_PREFIX_KEY), &[2; 40]);

            drop(original);
            assert_eq!(fork.prune(cutoff), 0);
            assert_eq!(fork.root(epoch(2)), root);
            assert_eq!(leaf_value(&fork, EDITED_KEY), &[1; 40]);
            let subtrie_idx = subtrie_index(&fork);
            let ArenaSparseNode::Subtrie(subtrie) = &mut fork.upper_arena[subtrie_idx] else {
                unreachable!()
            };
            assert!(!subtrie.arena.is_snapshot_ready());
            assert_eq!(subtrie.arena.len(), 1);
            assert_eq!(subtrie.root, NodeIndex::new(0));
            assert!(!subtrie.arena.has_vacant_slots());
        }
    }

    #[test]
    fn pruning_dense_exclusive_arenas_preserves_storage() {
        let mut trie = trie_with_shared_prefix_leaves();
        let cutoff = epoch(1);
        let root = trie.root(cutoff);
        assert_eq!(trie.prune(cutoff), 0);

        let subtrie_idx = trie
            .upper_arena
            .iter()
            .find_map(|(idx, node)| match node {
                ArenaSparseNode::Subtrie(subtrie)
                    if subtrie.path == Nibbles::from_nibbles([1, 2]) =>
                {
                    Some(idx)
                }
                _ => None,
            })
            .unwrap();
        let ArenaSparseNode::Subtrie(subtrie) = &mut trie.upper_arena[subtrie_idx] else {
            unreachable!()
        };
        assert!(!subtrie.arena.has_vacant_slots());
        let leaf = core::ptr::from_ref(&subtrie.arena[subtrie.root]);
        assert_eq!(subtrie.prune(cutoff), 0);
        assert_eq!(core::ptr::from_ref(&subtrie.arena[subtrie.root]), leaf);

        // A same-slot write needs reclamation even when it does not create vacancies.
        let peer = subtrie.arena.snapshot();
        subtrie.arena[subtrie.root] = peer[subtrie.root].clone();
        drop(peer);
        assert!(!subtrie.arena.has_vacant_slots());
        assert!(subtrie.arena.is_snapshot_ready());
        assert_eq!(subtrie.prune(cutoff), 0);
        assert!(!subtrie.arena.is_snapshot_ready());

        assert!(!trie.upper_arena.has_vacant_slots());
        let upper_root = core::ptr::from_ref(&trie.upper_arena[trie.root]);
        for cleanup_epoch in [cutoff, TrieNodeEpoch::UNMODIFIED] {
            assert_eq!(trie.prune(cleanup_epoch), 0);
            assert_eq!(core::ptr::from_ref(&trie.upper_arena[trie.root]), upper_root);
            assert_eq!(trie.root(cutoff), root);
        }
    }

    #[test]
    fn rolling_snapshots_preserve_roots_across_arena_boundary() {
        let mut current = PersistentArenaParallelSparseTrie::default();
        let initial: [(B256, &[u8]); 3] =
            [(EDITED_KEY, &[1; 40]), (SHARED_PREFIX_KEY, &[2; 40]), (DISTANT_BRANCH_KEY, &[3; 40])];
        apply_leaf_edits(&mut current, &initial);
        let mut leaves = BTreeMap::from(initial);
        assert_matches_leaves(&mut current, &leaves, TrieNodeEpoch::new(1));

        // Inserting the adjacent prefix splits the 0x11 branch at depth 1 and moves it to a
        // lower arena. Deleting it collapses that branch back into the upper arena. The
        // distant prefix keeps the top-level branch intact.
        let edits: [[(B256, &[u8]); 2]; 4] = [
            [(ADJACENT_PREFIX_KEY, &[4; 40]), (EDITED_KEY, &[5; 40])],
            [(ADJACENT_PREFIX_KEY, &[]), (SHARED_PREFIX_KEY, &[6; 40])],
            [(ADJACENT_PREFIX_KEY, &[7; 40]), (EDITED_KEY, &[8; 40])],
            [(ADJACENT_PREFIX_KEY, &[]), (SHARED_PREFIX_KEY, &[9; 40])],
        ];
        let mut history = Vec::new();
        for (epoch, edits) in (2..).zip(edits) {
            let epoch = TrieNodeEpoch::new(epoch);
            let mut next = current.snapshot();
            let mut next_leaves = leaves.clone();

            apply_leaf_edits(&mut current, &[(SHARED_PREFIX_KEY, &[])]);
            leaves.remove(&SHARED_PREFIX_KEY);
            apply_leaf_edits(&mut next, &edits);
            for (key, value) in edits {
                if value.is_empty() {
                    next_leaves.remove(&key);
                } else {
                    next_leaves.insert(key, value);
                }
            }

            assert_matches_leaves(&mut current, &leaves, epoch);
            assert_matches_leaves(&mut next, &next_leaves, epoch);
            history.push((current, leaves));
            (current, leaves) = (next, next_leaves);
        }

        for (mut trie, leaves) in history {
            assert_matches_leaves(&mut trie, &leaves, TrieNodeEpoch::new(6));
        }
    }

    #[test]
    fn clone_copies_nodes_even_after_taking_a_snapshot() {
        let mut original = trie_with_shared_prefix_leaves();
        let mut snapshot = original.snapshot();
        let mut cloned = snapshot.clone();
        for key in [EDITED_KEY, SHARED_PREFIX_KEY, ADJACENT_PREFIX_KEY] {
            assert!(core::ptr::eq(leaf_value(&original, key), leaf_value(&snapshot, key)));
            assert!(!core::ptr::eq(leaf_value(&snapshot, key), leaf_value(&cloned, key)));
        }
        apply_leaf_edits(&mut cloned, &[(EDITED_KEY, &[8; 40])]);
        assert_eq!(leaf_value(&original, EDITED_KEY), &[1; 40]);
        assert_eq!(leaf_value(&snapshot, EDITED_KEY), &[1; 40]);
        assert_eq!(leaf_value(&cloned, EDITED_KEY), &[8; 40]);
        let epoch = TrieNodeEpoch::default();
        assert_eq!(original.root(epoch), snapshot.root(epoch));
        assert_ne!(snapshot.root(epoch), cloned.root(epoch));
    }

    #[test]
    fn dirty_snapshots_preserve_root_epoch_and_pending_updates() {
        let mut original = trie_with_shared_prefix_leaves();
        original.root(TrieNodeEpoch::new(1));
        apply_leaf_edits(&mut original, &[(EDITED_KEY, &[5; 40])]);
        assert!(!original.updates_ref().updated_nodes.is_empty());
        assert!(original.buffers.rlp_node_buf.capacity() > 0);

        // Clone before snapshotting so both controls retain dense storage.
        let mut dense_original = original.clone();
        let mut dense_snapshot = original.clone();
        let mut snapshot = original.snapshot();
        assert_eq!(snapshot.buffers.rlp_node_buf.capacity(), 0);
        let epoch = TrieNodeEpoch::new(2);

        for (trie, dense, value) in [
            (&mut original, &mut dense_original, &[6; 40]),
            (&mut snapshot, &mut dense_snapshot, &[7; 40]),
        ] {
            assert_eq!(trie.root_epoch(), None);
            assert_eq!(trie.updates_ref(), dense.updates_ref());
            apply_leaf_edits(trie, &[(SHARED_PREFIX_KEY, value)]);
            apply_leaf_edits(dense, &[(SHARED_PREFIX_KEY, value)]);

            assert_eq!(trie.root(epoch), dense.root(epoch));
            assert_eq!(trie.root_epoch(), Some(epoch));
            assert_eq!(trie.root_epoch(), dense.root_epoch());
            assert_eq!(trie.take_updates(), dense.take_updates());
        }
    }

    fn assert_matches_leaves(
        trie: &mut PersistentArenaParallelSparseTrie,
        leaves: &BTreeMap<B256, &[u8]>,
        epoch: TrieNodeEpoch,
    ) {
        let mut builder = HashBuilder::default();
        for (&key, value) in leaves {
            builder.add_leaf(Nibbles::unpack(key), value);
        }
        assert_eq!(trie.root(epoch), builder.root(), "root at epoch {epoch:?}");
        for key in [EDITED_KEY, SHARED_PREFIX_KEY, ADJACENT_PREFIX_KEY, DISTANT_BRANCH_KEY] {
            assert_eq!(
                trie.get_leaf_value(&Nibbles::unpack(key)).map(Vec::as_slice),
                leaves.get(&key).copied(),
                "leaf {key} at epoch {epoch:?}"
            );
        }
    }

    #[test]
    fn snapshot_reveals_duplicate_extension_pairs_in_parallel_subtries() {
        let storage = [(0x11, 0x12), (0x91, 0x92)]
            .into_iter()
            .flat_map(|(sibling, prefix)| {
                [
                    (B256::right_padding_from(&[sibling]), U256::ONE),
                    (B256::right_padding_from(&[prefix, 0x80]), U256::from(2)),
                    (B256::right_padding_from(&[prefix, 0x34, 0x51]), U256::from(3)),
                    (B256::right_padding_from(&[prefix, 0x34, 0x52]), U256::from(4)),
                    (B256::right_padding_from(&[prefix, 0x34, 0x51, 0x10]), U256::from(5)),
                ]
            })
            .collect();
        let harness = ArenaTrieTestHarness::new(storage);
        let keys = [
            B256::right_padding_from(&[0x12, 0x34, 0x60]),
            B256::right_padding_from(&[0x92, 0x34, 0x60]),
        ];
        let changes = keys.into_iter().map(|key| (key, U256::from(9))).collect();
        let (expected_root, mut expected_updates) = harness.get_root_with_updates(&changes);
        harness.minimize_trie_updates(&mut expected_updates);

        for min_revealed_nodes in [0, usize::MAX] {
            let mut original = PersistentArenaParallelSparseTrie::default()
                .with_parallelism_thresholds(PersistentArenaParallelismThresholds {
                    min_revealed_nodes,
                    ..Default::default()
                });
            original.set_root(&mut harness.root_nodes(), true).unwrap();
            let mut targets =
                harness.storage().keys().copied().map(ProofV2Target::new).collect::<Vec<_>>();
            let (mut upper_nodes, _) = harness.proof_v2(&mut targets);
            // Reveal the lower roots, but leave their extension children blinded.
            upper_nodes.retain(|node| node.path.len() <= UPPER_TRIE_MAX_DEPTH);
            original.reveal_nodes(&mut upper_nodes).unwrap();
            assert_eq!(original.root(epoch(0)), harness.original_root());
            assert_eq!(original.take_updates(), SparseTrieUpdates::default());
            for key in keys {
                let prefix = Nibbles::unpack(key).slice(..UPPER_TRIE_MAX_DEPTH);
                let subtrie = original
                    .upper_arena
                    .iter()
                    .find_map(|(_, node)| match node {
                        ArenaSparseNode::Subtrie(subtrie) if subtrie.path == prefix => {
                            Some(subtrie)
                        }
                        _ => None,
                    })
                    .expect("each target has an existing depth-2 subtrie");
                assert!(matches!(subtrie.arena[subtrie.root], ArenaSparseNode::Branch(_)));
                assert!(matches!(
                    original.find_leaf(&Nibbles::unpack(key), None),
                    Err(LeafLookupError::BlindedNode { .. })
                ));
            }
            let mut snapshot = original.snapshot();
            let mut targets =
                keys.map(|key| ProofV2Target::new(key).with_parent(ProofV2TargetParent::new(2)));
            let (mut proof, _) = harness.proof_v2(&mut targets);
            assert_eq!(proof.len(), 4, "two lower extension/branch pairs");
            for key in keys {
                let path = Nibbles::unpack(key);
                let extension = proof.iter().find(|node| node.path == path.slice(..3)).unwrap();
                let TrieNode::Extension(extension) = &extension.node else {
                    panic!("each lower proof starts with an extension");
                };
                assert_eq!(extension.key, Nibbles::from_nibbles([4, 5]));
                assert!(extension.child.is_hash());
                let branch_path = path.slice(..4).join(&Nibbles::from_nibbles([5]));
                let branch = proof.iter().find(|node| node.path == branch_path).unwrap();
                assert!(matches!(branch.node, TrieNode::Branch(_)));
                assert!(
                    !branch.masks.unwrap().is_empty(),
                    "the child branch has persisted descendants"
                );
            }
            let mut overlapping = proof.clone();
            for node in &mut overlapping {
                node.masks = None;
            }
            proof.extend(overlapping);
            snapshot.reveal_nodes(&mut proof).unwrap();
            assert_eq!(snapshot.root(epoch(0)), harness.original_root());
            assert_eq!(snapshot.take_updates(), SparseTrieUpdates::default());
            for key in keys {
                assert_eq!(
                    snapshot.find_leaf(&Nibbles::unpack(key), None),
                    Ok(LeafLookup::NonExistent)
                );
                assert!(matches!(
                    original.find_leaf(&Nibbles::unpack(key), None),
                    Err(LeafLookupError::BlindedNode { .. })
                ));
            }

            apply_leaf_edits(&mut snapshot, &[(keys[0], &[9]), (keys[1], &[9])]);
            assert_eq!(snapshot.root(epoch(1)), expected_root);
            let mut updates = snapshot.take_updates();
            for key in keys {
                let parent = &updates.updated_nodes[&Nibbles::unpack(key).slice(..4)];
                assert_eq!(parent.hash_mask.count_bits(), 1);
                assert!(parent.hash_mask.is_bit_set(5));
                assert_eq!(parent.tree_mask, parent.hash_mask);
                assert_eq!(leaf_value(&snapshot, key), &[9]);
                assert!(matches!(
                    original.find_leaf(&Nibbles::unpack(key), None),
                    Err(LeafLookupError::BlindedNode { .. })
                ));
            }
            updates.updated_nodes.retain(|path, node| {
                harness.storage_trie_updates().storage_nodes.get(path) != Some(node)
            });
            updates
                .removed_nodes
                .retain(|path| harness.storage_trie_updates().storage_nodes.contains_key(path));
            assert_eq!(
                updates,
                SparseTrieUpdates {
                    updated_nodes: expected_updates.storage_nodes.clone().into_iter().collect(),
                    removed_nodes: expected_updates.removed_nodes.clone().into_iter().collect(),
                }
            );
            assert_eq!(original.root(epoch(1)), harness.original_root());
            assert_eq!(original.take_updates(), SparseTrieUpdates::default());
        }
    }

    #[test]
    fn opaque_extension_snapshot_hydration_keeps_cache_and_slot_isolation() {
        let harness = TrieTestHarness::new(BTreeMap::from([
            (EDITED_KEY, U256::ONE),
            (SHARED_PREFIX_KEY, U256::from(2)),
        ]));
        let mut root_nodes = harness.root_nodes();
        root_nodes.sort_unstable_by_key(|node| node.path);
        let root_proof = root_nodes.remove(0);
        let TrieNode::Extension(extension) = &root_proof.node else {
            panic!("fixture root must be an extension");
        };
        let backing_rlp = extension.child.clone();
        let mut source = PersistentArenaParallelSparseTrie::default();
        source.set_root(&mut [root_proof.clone()], true).unwrap();
        assert_eq!(source.upper_arena.len(), 1);
        assert!(has_blinded_branch(&source.upper_arena, source.root));
        assert_eq!(source.root(epoch(9)), harness.original_root());
        assert_eq!(source.root_epoch(), Some(TrieNodeEpoch::UNMODIFIED));

        let mut hydrated = source.snapshot();
        let mut opaque = source.snapshot();
        assert!(core::ptr::eq(
            &raw const source.upper_arena[source.root],
            &raw const opaque.upper_arena[opaque.root]
        ));
        assert_eq!(opaque.root(epoch(10)), harness.original_root());
        let opaque_root = opaque.root;
        PersistentArenaParallelSparseTrie::encode_blinded_extension(
            &mut opaque.upper_arena,
            opaque_root,
            &mut opaque.buffers.rlp_buf,
        );
        assert!(core::ptr::eq(
            &raw const source.upper_arena[source.root],
            &raw const opaque.upper_arena[opaque.root]
        ));
        let cached_extension = hydrated.upper_arena[hydrated.root].state_ref().cloned();
        // Replay the extension together with its backing proof, as overlapping batches do.
        let mut batch = vec![root_proof];
        batch.extend(root_nodes);
        hydrated.reveal_nodes(&mut batch).unwrap();
        assert_eq!(hydrated.upper_arena.len(), 2);
        assert_eq!(hydrated.upper_arena[hydrated.root].state_ref(), cached_extension.as_ref());
        let branch = hydrated.upper_arena[hydrated.root].extension_child().unwrap();
        assert_eq!(
            hydrated.upper_arena[branch].state_ref(),
            Some(&ArenaSparseNodeState::Cached {
                rlp_node: backing_rlp,
                epoch: TrieNodeEpoch::UNMODIFIED,
            })
        );
        assert_eq!(hydrated.root(epoch(10)), harness.original_root());
        assert_eq!(hydrated.take_updates(), SparseTrieUpdates::default());
        assert_eq!(opaque.upper_arena.len(), 1);
        assert!(has_blinded_branch(&opaque.upper_arena, opaque.root));
        assert!(has_blinded_branch(&source.upper_arena, source.root));

        // Exclusive compaction handles both opaque and hydrated extension children.
        let mut dense_opaque = opaque.clone();
        dense_opaque.upper_arena.compact(&mut dense_opaque.root);
        assert_eq!(dense_opaque.upper_arena.len(), 1);
        assert_eq!(dense_opaque.root(epoch(11)), harness.original_root());
        let mut dense_hydrated = hydrated.clone();
        dense_hydrated.upper_arena.compact(&mut dense_hydrated.root);
        assert_eq!(dense_hydrated.upper_arena.len(), 2);
        assert_eq!(dense_hydrated.root(epoch(11)), harness.original_root());
        drop(hydrated);
        drop(source);
        assert_eq!(opaque.prune(epoch(11)), 0);
        assert_eq!(opaque.root(epoch(11)), harness.original_root());
    }

    #[test]
    fn lower_pruning_retains_and_releases_opaque_extensions_across_snapshots() {
        for shared in [false, true] {
            let mut subtrie = ArenaSparseSubtrie::new(false);
            let opaque_branch = RlpNode::word_rlp(&B256::repeat_byte(4));
            let old_rlp = RlpNode::word_rlp(&B256::repeat_byte(5));
            let retained_rlp = RlpNode::word_rlp(&B256::repeat_byte(6));
            let old = subtrie.arena.insert(ArenaSparseNode::Extension {
                key: Nibbles::from_nibbles([4]),
                child: ExtensionChild::Blinded(opaque_branch.clone()),
                state: ArenaSparseNodeState::Cached { rlp_node: old_rlp.clone(), epoch: epoch(1) },
            });
            let retained = subtrie.arena.insert(ArenaSparseNode::Extension {
                key: Nibbles::from_nibbles([5]),
                child: ExtensionChild::Blinded(opaque_branch.clone()),
                state: ArenaSparseNodeState::Cached { rlp_node: retained_rlp, epoch: epoch(10) },
            });
            subtrie.arena[subtrie.root] = ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                state: ArenaSparseNodeState::Cached {
                    rlp_node: RlpNode::word_rlp(&B256::repeat_byte(7)),
                    epoch: epoch(10),
                },
                children: [BranchChild::revealed(old), BranchChild::revealed(retained)]
                    .into_iter()
                    .collect(),
                state_mask: TrieMask::from_nibble(1) | TrieMask::from_nibble(2),
                branch_masks: BranchNodeMasks::default(),
            });
            let peer = shared.then(|| subtrie.arena.snapshot());
            assert_eq!(subtrie.prune(epoch(5)), 1);
            assert_eq!(subtrie.arena.len(), 2);
            let branch = subtrie.arena[subtrie.root].branch_ref();
            assert_eq!(subtrie.arena.blinded(branch.children[0]), &old_rlp);
            let retained = branch.children[1].revealed_index().unwrap();
            assert!(matches!(&subtrie.arena[retained], ArenaSparseNode::Extension {
                child: ExtensionChild::Blinded(rlp), ..
            } if *rlp == opaque_branch));
            if let Some(peer) = peer {
                assert_eq!(peer.len(), 3);
                assert!(has_blinded_branch(&peer, old));
            }
            assert_eq!(subtrie.prune(epoch(5)), 0);
            assert_eq!(subtrie.arena.len(), 2);
            #[cfg(all(debug_assertions, not(feature = "fuzz")))]
            PersistentArenaParallelSparseTrie::assert_no_orphaned_nodes(
                &subtrie.arena,
                subtrie.root,
                "retained opaque extension",
            );
        }
    }
}
