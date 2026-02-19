use crate::{
    LeafLookup, LeafLookupError, LeafUpdate, SparseTrie, SparseTrieUpdates, SparseTrieUpdatesAction,
};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_primitives::{map::B256Map, B256};
use alloy_trie::TrieMask;
use core::mem;
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};
use smallvec::SmallVec;
use thunderdome::{Arena, Index};

/// The maximum path length (in nibbles) for nodes that live in the upper trie. Nodes at this
/// depth or deeper belong to lower subtries.
const UPPER_TRIE_MAX_DEPTH: usize = 2;

/// Tracks whether a node's RLP encoding is cached or needs recomputation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArenaSparseNodeState {
    /// The node has been revealed but its RLP encoding is not cached.
    Revealed {
        /// Total number of revealed leaves underneath this node (recursively).
        num_leaves: u64,
    },
    /// The node has a cached RLP encoding that is still valid.
    Cached {
        /// The cached RLP-encoded representation of the node.
        rlp_node: RlpNode,
        /// Total number of revealed leaves underneath this node (recursively).
        num_leaves: u64,
    },
    /// The node has been modified and its RLP encoding needs recomputation.
    Dirty {
        /// Total number of revealed leaves underneath this node (recursively).
        num_leaves: u64,
        /// Number of dirty (modified since last hash) leaves underneath this node.
        num_dirty_leaves: u64,
    },
}

impl ArenaSparseNodeState {
    /// Converts into a [`Self::Dirty`] if it's not already, preserving the `num_leaves` field.
    fn to_dirty(&self) -> Self {
        match self {
            Self::Revealed { num_leaves, .. } | Self::Cached { num_leaves, .. } => {
                Self::Dirty { num_leaves: *num_leaves, num_dirty_leaves: 0 }
            }
            Self::Dirty { .. } => self.clone(),
        }
    }

    const fn num_leaves(&self) -> u64 {
        match self {
            Self::Revealed { num_leaves } |
            Self::Cached { num_leaves, .. } |
            Self::Dirty { num_leaves, .. } => *num_leaves,
        }
    }

    /// Applies a delta to the state's `num_leaves` field, returning the new value.
    const fn add_num_leaves(&mut self, delta: i64) -> u64 {
        match self {
            Self::Revealed { num_leaves } |
            Self::Cached { num_leaves, .. } |
            Self::Dirty { num_leaves, .. } => {
                *num_leaves = (*num_leaves as i64 + delta) as u64;
                *num_leaves
            }
        }
    }

    /// Returns the `num_dirty_leaves` count, or 0 if not dirty.
    const fn num_dirty_leaves(&self) -> u64 {
        match self {
            Self::Dirty { num_dirty_leaves, .. } => *num_dirty_leaves,
            _ => 0,
        }
    }
}

/// Represents a reference from a branch node to one of its children.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArenaSparseNodeBranchChild {
    /// The child node has been revealed and is present in the arena.
    Revealed(Index),
    /// The child node has not been revealed; only its RLP-encoded node is known.
    Blinded(RlpNode),
}

impl ArenaSparseNodeBranchChild {
    const fn is_blinded(&self) -> bool {
        matches!(self, Self::Blinded(_))
    }
}

/// The branch-specific data stored in an [`ArenaSparseNode::Branch`].
#[derive(Debug)]
struct ArenaSparseNodeBranch {
    /// Cached or dirty state of this node.
    state: ArenaSparseNodeState,
    /// Revealed or blinded children, packed densely. The `state_mask` tracks which
    /// nibble positions have entries in this `SmallVec`.
    children: SmallVec<[ArenaSparseNodeBranchChild; 4]>,
    /// Bitmask indicating which of the 16 child slots are occupied (have an entry
    /// in `children`).
    state_mask: TrieMask,
    /// The short key (extension key) for this branch. When non-empty, the node's path is the
    /// path of the parent extension node with this short key.
    short_key: Nibbles,
    /// Tree mask and hash mask for database persistence (`TrieUpdates`).
    #[expect(dead_code)]
    branch_masks: BranchNodeMasks,
}

/// A node in the arena-based sparse trie.
#[derive(Debug)]
enum ArenaSparseNode {
    /// Indicates a trie with no nodes.
    EmptyRoot,
    /// A branch node with up to 16 children.
    Branch(ArenaSparseNodeBranch),
    /// A leaf node containing a value.
    Leaf {
        /// Cached or dirty state of this node.
        state: ArenaSparseNodeState,
        /// The RLP-encoded leaf value.
        value: Vec<u8>,
        /// The remaining key suffix for this leaf.
        key: Nibbles,
    },
    /// A subtrie that can be taken for parallel processing.
    Subtrie(Box<ArenaSparseSubtrie>),
    /// Placeholder for a subtrie that has been temporarily taken for parallel operations.
    TakenSubtrie,
}

impl ArenaSparseNode {
    fn num_leaves(&self) -> u64 {
        match self {
            Self::EmptyRoot | Self::TakenSubtrie => 0,
            Self::Branch(b) => b.state.num_leaves(),
            Self::Leaf { state, .. } => state.num_leaves(),
            Self::Subtrie(s) => s.arena[s.root].num_leaves(),
        }
    }

    fn num_dirty_leaves(&self) -> u64 {
        match self {
            Self::EmptyRoot | Self::TakenSubtrie => 0,
            Self::Branch(b) => b.state.num_dirty_leaves(),
            Self::Leaf { state, .. } => state.num_dirty_leaves(),
            Self::Subtrie(s) => s.arena[s.root].num_dirty_leaves(),
        }
    }

    fn state_mut(&mut self) -> &mut ArenaSparseNodeState {
        match self {
            Self::Branch(b) => &mut b.state,
            Self::Leaf { state, .. } => state,
            _ => panic!("state_mut called on non-Branch/Leaf node"),
        }
    }

    #[expect(dead_code)]
    const fn short_key_len(&self) -> usize {
        match self {
            Self::Branch(b) => b.short_key.len(),
            _ => 0,
        }
    }

    fn branch_ref(&self) -> &ArenaSparseNodeBranch {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch_ref called on non-Branch node {self:?}"),
        }
    }

    fn branch_mut(&mut self) -> &mut ArenaSparseNodeBranch {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch_mut called on non-Branch node {self:?}"),
        }
    }
}

impl From<ProofTrieNodeV2> for ArenaSparseNode {
    fn from(proof_node: ProofTrieNodeV2) -> Self {
        let ProofTrieNodeV2 { node, masks, .. } = proof_node;
        match node {
            TrieNodeV2::EmptyRoot => Self::EmptyRoot,
            TrieNodeV2::Leaf(leaf) => Self::Leaf {
                state: ArenaSparseNodeState::Revealed { num_leaves: 1 },
                key: leaf.key,
                value: leaf.value,
            },
            TrieNodeV2::Branch(branch) => {
                let mut children = SmallVec::with_capacity(branch.state_mask.count_bits() as usize);
                for (stack_ptr, _nibble) in branch.state_mask.iter().enumerate() {
                    children
                        .push(ArenaSparseNodeBranchChild::Blinded(branch.stack[stack_ptr].clone()));
                }
                Self::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed { num_leaves: 0 },
                    children,
                    state_mask: branch.state_mask,
                    short_key: branch.key,
                    branch_masks: masks.unwrap_or_default(),
                })
            }
            TrieNodeV2::Extension(_) => {
                panic!("Extension nodes should be merged into branches by TrieNodeV2")
            }
        }
    }
}

/// Right-pads a nibble path with zeros and packs it into a [`B256`].
fn nibbles_to_padded_b256(path: &Nibbles) -> B256 {
    let mut bytes = [0u8; 32];
    path.pack_to(&mut bytes);
    B256::from(bytes)
}

/// Returns the index into a densely-packed children array for a given nibble, based on the
/// state mask. Returns `None` if the nibble's bit is not set.
const fn child_dense_index(state_mask: TrieMask, nibble: u8) -> Option<usize> {
    if !state_mask.is_bit_set(nibble) {
        return None;
    }
    Some((state_mask.get() & ((1u16 << nibble) - 1)).count_ones() as usize)
}

/// An entry on the traversal stack, tracking an ancestor branch during trie walks.
#[derive(Debug, Clone)]
struct ArenaStackEntry {
    /// The arena index of this branch node.
    index: Index,
    /// The absolute path of this branch node in the trie (not including its `short_key`).
    path: Nibbles,
    /// The `num_leaves` value when this branch was pushed onto the stack. Used to compute the
    /// delta to propagate to the parent when popped.
    prev_num_leaves: u64,
    /// The `num_dirty_leaves` value when this branch was pushed onto the stack. Used to compute
    /// the delta to propagate to the parent when popped.
    prev_dirty_leaves: u64,
}

/// Result of [`find_ancestor`] describing the state at the deepest ancestor node.
#[derive(Debug)]
enum FindAncestorResult {
    /// The stack head is an empty root node.
    EmptyRoot,
    /// The stack head is a leaf node. The leaf has been pushed onto the stack.
    RevealedLeaf,
    /// The next child along the path is blinded (unrevealed).
    Blinded,
    /// The target path diverges within the stack head branch's `short_key`.
    Diverged,
    /// The target nibble has no child in the branch's `state_mask`.
    NoChild { child_nibble: u8 },
    /// The target nibble has a revealed subtrie child at the given dense index.
    #[expect(dead_code)]
    RevealedSubtrie { dense_idx: usize, child_nibble: u8, child_idx: Index },
}

/// Pops the stack until the head is an ancestor of `full_path`, then descends from that head
/// toward `full_path`, pushing revealed branch (and leaf) children onto the stack until the
/// deepest ancestor is reached.
///
/// Returns a [`FindAncestorResult`] describing the state at the stack head.
fn find_ancestor(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
    full_path: &Nibbles,
) -> FindAncestorResult {
    // Pop stack until head is ancestor of full_path.
    while stack.len() > 1 && !full_path.starts_with(&stack.last().unwrap().path) {
        pop_and_propagate(arena, stack);
    }

    loop {
        let head = stack.last().unwrap();
        let head_idx = head.index;

        let head_branch = match &arena[head_idx] {
            ArenaSparseNode::EmptyRoot => return FindAncestorResult::EmptyRoot,
            ArenaSparseNode::Leaf { .. } => return FindAncestorResult::RevealedLeaf,
            ArenaSparseNode::Branch(head_branch) => head_branch,
            ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie => {
                unreachable!("unexpected node type on stack: {:?}", arena[head_idx])
            }
        };

        let mut head_branch_logical_path = head.path;
        head_branch_logical_path.extend(&head_branch.short_key);

        // If full_path doesn't extend past the branch's logical path, the target is at or
        // within the branch's short_key — treat as diverged.
        if full_path.len() <= head_branch_logical_path.len() ||
            !full_path.starts_with(&head_branch_logical_path)
        {
            return FindAncestorResult::Diverged;
        }

        let child_nibble = full_path.get_unchecked(head_branch_logical_path.len());
        let Some(dense_idx) = child_dense_index(head_branch.state_mask, child_nibble) else {
            return FindAncestorResult::NoChild { child_nibble };
        };

        match &head_branch.children[dense_idx] {
            ArenaSparseNodeBranchChild::Blinded(_) => return FindAncestorResult::Blinded,
            ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                let child_idx = *child_idx;
                if maybe_push_child(arena, stack, child_idx, child_nibble) {
                    // Child was pushed; if it's a leaf we stop, if a branch we continue.
                    if matches!(arena[child_idx], ArenaSparseNode::Leaf { .. }) {
                        return FindAncestorResult::RevealedLeaf;
                    }
                } else {
                    return FindAncestorResult::RevealedSubtrie {
                        dense_idx,
                        child_nibble,
                        child_idx,
                    };
                }
            }
        }
    }
}

/// Returns the absolute path of a child at `child_nibble` under the branch at the top of the
/// stack. The result is `stack_head.path + branch.short_key + child_nibble`.
fn child_path(
    arena: &Arena<ArenaSparseNode>,
    stack: &[ArenaStackEntry],
    child_nibble: u8,
) -> Nibbles {
    let parent = stack.last().unwrap();
    let mut path = parent.path;
    path.extend(&arena[parent.index].branch_ref().short_key);
    path.push_unchecked(child_nibble);
    path
}

/// If `child_idx` is a branch or leaf node, pushes it onto the stack and returns `true`.
/// Returns `false` for subtries and other non-pushable node types.
///
/// The parent's path and `short_key` are read from the current top of the stack.
fn maybe_push_child(
    arena: &Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
    child_idx: Index,
    child_nibble: u8,
) -> bool {
    let node = &arena[child_idx];
    match node {
        ArenaSparseNode::Branch(b) => {
            let path = child_path(arena, stack, child_nibble);
            stack.push(ArenaStackEntry {
                index: child_idx,
                path,
                prev_num_leaves: b.state.num_leaves(),
                prev_dirty_leaves: b.state.num_dirty_leaves(),
            });
            true
        }
        ArenaSparseNode::Leaf { state, .. } => {
            let path = child_path(arena, stack, child_nibble);
            stack.push(ArenaStackEntry {
                index: child_idx,
                path,
                prev_num_leaves: state.num_leaves(),
                prev_dirty_leaves: state.num_dirty_leaves(),
            });
            true
        }
        _ => false,
    }
}

/// Assert that the popped branch's leaf counts are consistent with its revealed children.
/// No-op for non-branch nodes (e.g. leaves on the stack).
#[cfg(debug_assertions)]
fn debug_assert_branch_consistency(arena: &Arena<ArenaSparseNode>, branch_idx: Index) {
    let ArenaSparseNode::Branch(b) = &arena[branch_idx] else {
        return;
    };

    let revealed_num_leaves: u64 = b
        .children
        .iter()
        .filter_map(|c| match c {
            ArenaSparseNodeBranchChild::Revealed(idx) => Some(arena[*idx].num_leaves()),
            ArenaSparseNodeBranchChild::Blinded(_) => None,
        })
        .sum();
    debug_assert_eq!(b.state.num_leaves(), revealed_num_leaves, "branch num_leaves mismatch",);

    let revealed_dirty_leaves: u64 = b
        .children
        .iter()
        .filter_map(|c| match c {
            ArenaSparseNodeBranchChild::Revealed(idx) => Some(arena[*idx].num_dirty_leaves()),
            ArenaSparseNodeBranchChild::Blinded(_) => None,
        })
        .sum();
    debug_assert_eq!(
        b.state.num_dirty_leaves(),
        revealed_dirty_leaves,
        "branch num_dirty_leaves mismatch",
    );
}

/// Pops the top entry from the stack and propagates its `num_leaves` and dirty state/count
/// deltas to the new top (its parent). Returns the popped entry.
fn pop_and_propagate(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
) -> Option<ArenaStackEntry> {
    let entry = stack.pop()?;

    #[cfg(debug_assertions)]
    debug_assert_branch_consistency(arena, entry.index);

    if let Some(parent) = stack.last() {
        let child = &arena[entry.index];
        let cur_num_leaves = child.num_leaves();
        let cur_dirty_leaves = child.num_dirty_leaves();

        let parent_state = arena[parent.index].state_mut();
        let leaves_delta = cur_num_leaves as i64 - entry.prev_num_leaves as i64;

        // If a child has any dirty leaves, the parent must become Dirty too. It's sufficient to
        // check the latest dirty leaf count; if the previous count was >0, but this went to zero,
        // it would indicate the branch was deleted, and so wouldn't be on the stack.
        if cur_dirty_leaves > 0 {
            let dirty_delta = cur_dirty_leaves as i64 - entry.prev_dirty_leaves as i64;
            *parent_state = ArenaSparseNodeState::Dirty {
                num_leaves: (parent_state.num_leaves() as i64 + leaves_delta) as u64,
                num_dirty_leaves: (parent_state.num_dirty_leaves() as i64 + dirty_delta) as u64,
            };
        } else if leaves_delta != 0 {
            parent_state.add_num_leaves(leaves_delta);
        }
    }
    Some(entry)
}

/// Drains the stack, propagating `num_leaves` deltas from each entry to its parent.
fn drain_stack(arena: &mut Arena<ArenaSparseNode>, stack: &mut Vec<ArenaStackEntry>) {
    while stack.len() > 1 {
        pop_and_propagate(arena, stack);
    }
}

/// A subtrie within the arena-based parallel sparse trie.
///
/// Each subtrie owns its own arena, allowing parallel mutations across subtries.
#[derive(Debug)]
struct ArenaSparseSubtrie {
    /// The arena allocating nodes within this subtrie.
    arena: Arena<ArenaSparseNode>,
    /// The root node of this subtrie.
    root: Index,
    /// Reusable stack buffer for trie traversals.
    stack: Vec<ArenaStackEntry>,
    /// Reusable buffer for collecting update actions during hash computations.
    update_actions: Vec<SparseTrieUpdatesAction>,
    /// Reusable buffer for collecting required proofs during leaf updates.
    required_proofs: Vec<ArenaRequiredProof>,
}

impl ArenaSparseSubtrie {
    fn clear(&mut self) {
        self.arena.clear();
        self.stack.clear();
        self.update_actions.clear();
        self.required_proofs.clear();
    }

    /// Applies leaf updates within this subtrie. Uses the same walk-down-with-stack pattern as
    /// [`Self::reveal_nodes`], but checks accessibility for [`LeafUpdate::Touched`] entries.
    ///
    /// `root_path` is the absolute path of the subtrie's root node in the full trie.
    /// `sorted_updates` must be sorted lexicographically by their nibbles path (index 1).
    ///
    /// Any required proofs are appended to `self.required_proofs` and should be drained by the
    /// caller after this method returns.
    fn update_leaves(
        &mut self,
        sorted_updates: &[(B256, Nibbles, LeafUpdate)],
        root_path: Nibbles,
    ) {
        if sorted_updates.is_empty() {
            return;
        }

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot),
            "subtrie root must not be EmptyRoot at start of update_leaves"
        );

        self.stack.clear();
        let root_node = &self.arena[self.root];
        self.stack.push(ArenaStackEntry {
            index: self.root,
            path: root_path,
            prev_num_leaves: root_node.num_leaves(),
            prev_dirty_leaves: root_node.num_dirty_leaves(),
        });

        for &(key, ref full_path, ref update) in sorted_updates {
            let find_result = find_ancestor(&mut self.arena, &mut self.stack, full_path);

            // If the path hits a blinded node, request a proof regardless of update type.
            if matches!(find_result, FindAncestorResult::Blinded) {
                let head = self.stack.last().unwrap();
                let head_branch = self.arena[head.index].branch_ref();
                let logical_len = head.path.len() + head_branch.short_key.len();
                self.required_proofs
                    .push(ArenaRequiredProof { key, min_len: (logical_len as u8 + 1).min(64) });
                continue;
            }

            match update {
                LeafUpdate::Changed(value) if !value.is_empty() => {
                    // Upsert: insert or update a leaf with the given value.
                    upsert_leaf(
                        &mut self.arena,
                        &mut self.stack,
                        &mut self.root,
                        full_path,
                        value,
                        find_result,
                    );
                }
                LeafUpdate::Changed(_) => {
                    let result = remove_leaf(
                        &mut self.arena,
                        &mut self.stack,
                        &mut self.root,
                        key,
                        full_path,
                        find_result,
                    );
                    if let RemoveLeafResult::NeedsProof { key, proof_key, min_len } = result {
                        self.required_proofs.push(ArenaRequiredProof { key: proof_key, min_len });
                        self.required_proofs.push(ArenaRequiredProof { key, min_len });
                    }
                }
                LeafUpdate::Touched => {}
            }
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.arena, &mut self.stack);
    }

    /// Reveals nodes inside this subtrie. Uses [`find_ancestor`] to locate the ancestor
    /// node, then replaces blinded children with the proof nodes.
    ///
    /// `root_path` is the absolute path of the subtrie's root node in the full trie (not
    /// including the root's `short_key`).
    fn reveal_nodes(
        &mut self,
        nodes: &mut [ProofTrieNodeV2],
        root_path: Nibbles,
    ) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot),
            "subtrie root must not be EmptyRoot in reveal_nodes"
        );

        let root_num_leaves = self.arena[self.root].num_leaves();

        self.stack.clear();
        self.stack.push(ArenaStackEntry {
            index: self.root,
            path: root_path,
            prev_num_leaves: root_num_leaves,
            prev_dirty_leaves: 0,
        });

        for node in nodes.iter_mut() {
            let find_result = find_ancestor(&mut self.arena, &mut self.stack, &node.path);
            reveal_node(&mut self.arena, &mut self.stack, node, find_result);
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.arena, &mut self.stack);

        Ok(())
    }
}

/// Creates a new leaf and a new branch that splits an existing child from the new leaf at
/// a divergence point. Returns the index of the new branch.
///
/// `new_leaf_path` is the full remaining path for the new leaf (relative to the split
/// point's parent). `old_child_path` is the old child's remaining path (its leaf key or
/// branch `short_key`). The divergence point is computed as the common prefix length of the
/// two paths. `old_child_idx` is the arena index of the existing child being split away.
///
/// The old child's key (leaf) or `short_key` (branch) is truncated to the suffix after the
/// divergence nibble and its state is set to dirty.
fn split_and_insert_leaf(
    arena: &mut Arena<ArenaSparseNode>,
    new_leaf_path: Nibbles,
    value: &[u8],
    old_child_path: &Nibbles,
    old_child_idx: Index,
) -> Index {
    let diverge_len = new_leaf_path.common_prefix_length(old_child_path);
    let old_child_nibble = old_child_path.get_unchecked(diverge_len);
    let old_child_suffix = old_child_path.slice(diverge_len + 1..);

    // Truncate the old child's key/short_key and mark it dirty.
    match &mut arena[old_child_idx] {
        ArenaSparseNode::Leaf { key, state, .. } => {
            *key = old_child_suffix;
            *state = ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 };
        }
        ArenaSparseNode::Branch(b) => {
            b.short_key = old_child_suffix;
            b.state = b.state.to_dirty();
        }
        _ => unreachable!("split_and_insert_leaf called on non-Leaf/Branch node"),
    }

    let old_child_num_leaves = arena[old_child_idx].num_leaves();
    let old_child_dirty_leaves = arena[old_child_idx].num_dirty_leaves();

    let short_key = new_leaf_path.slice(..diverge_len);
    let new_leaf_nibble = new_leaf_path.get_unchecked(diverge_len);
    debug_assert_ne!(old_child_nibble, new_leaf_nibble);

    let new_leaf_idx = arena.insert(ArenaSparseNode::Leaf {
        state: ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 },
        key: new_leaf_path.slice(diverge_len + 1..),
        value: value.to_vec(),
    });

    let (first_nibble, first_child, second_nibble, second_child) =
        if old_child_nibble < new_leaf_nibble {
            (old_child_nibble, old_child_idx, new_leaf_nibble, new_leaf_idx)
        } else {
            (new_leaf_nibble, new_leaf_idx, old_child_nibble, old_child_idx)
        };

    let state_mask = TrieMask::from(1u16 << first_nibble | 1u16 << second_nibble);
    let mut children = SmallVec::with_capacity(2);
    children.push(ArenaSparseNodeBranchChild::Revealed(first_child));
    children.push(ArenaSparseNodeBranchChild::Revealed(second_child));

    arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
        state: ArenaSparseNodeState::Dirty {
            num_leaves: old_child_num_leaves + 1,
            num_dirty_leaves: old_child_dirty_leaves + 1,
        },
        children,
        state_mask,
        short_key,
        branch_masks: BranchNodeMasks::default(),
    }))
}

/// Replaces a child index in the parent's children array. The parent is the second-to-last
/// stack entry. If the stack has only one entry, `old_idx` is the trie root and `root` is
/// updated instead.
///
/// `old_idx` must be on the stack (it is the current stack head), so its nibble within
/// the parent can be determined from the last nibble of its path.
///
/// Returns the (possibly updated) root index.
fn replace_child_in_parent(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &[ArenaStackEntry],
    root: Index,
    old_idx: Index,
    new_idx: Index,
) -> Index {
    if stack.len() <= 1 {
        return new_idx;
    }

    let child_path = &stack.last().unwrap().path;
    let child_nibble = child_path.last().unwrap();

    let parent_entry = &stack[stack.len() - 2];
    let parent = arena[parent_entry.index].branch_mut();
    let dense_idx = child_dense_index(parent.state_mask, child_nibble)
        .expect("child nibble not found in parent state_mask");
    debug_assert!(
        matches!(parent.children[dense_idx], ArenaSparseNodeBranchChild::Revealed(idx) if idx == old_idx),
        "parent child at nibble {child_nibble} does not match old_idx",
    );
    parent.children[dense_idx] = ArenaSparseNodeBranchChild::Revealed(new_idx);
    root
}

/// Result of [`upsert_leaf`] indicating whether a new branch was created that the caller
/// may need to wrap as a subtrie (in the upper trie).
#[derive(Debug)]
enum UpsertLeafResult {
    /// No new branch was created (value update, new leaf only, or same-leaf overwrite).
    None,
    /// A new branch was created. Contains the parent's arena index, the child nibble under
    /// which the new branch sits, and the parent's path — enough for the caller to call
    /// `maybe_wrap_in_subtrie`.
    NewBranch { parent_idx: Index, child_nibble: u8, parent_path: Nibbles },
}

/// Performs a leaf upsert using a pre-computed [`FindAncestorResult`] from
/// [`find_ancestor`].
///
/// Handles three cases based on `find_result`:
/// 1. `RevealedLeaf` — the stack head is a leaf; update in place or split into a branch.
/// 2. Diverged — the path diverges within the branch's `short_key`, split it.
/// 3. `NoChild` — the target nibble has no child, insert a new leaf.
///
/// The caller must handle [`FindAncestorResult::Blinded`] and
/// [`FindAncestorResult::RevealedSubtrie`] before calling this function.
/// The stack must have at least one entry when called.
///
/// Returns an [`UpsertLeafResult`] describing any new branch that was created, so the caller
/// can decide whether to wrap it as a subtrie.
fn upsert_leaf(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut [ArenaStackEntry],
    root: &mut Index,
    full_path: &Nibbles,
    value: &[u8],
    find_result: FindAncestorResult,
) -> UpsertLeafResult {
    let head = stack.last().unwrap();

    match find_result {
        FindAncestorResult::Blinded => {
            unreachable!("Blinded case must be handled by caller")
        }
        FindAncestorResult::EmptyRoot => {
            let head_idx = head.index;
            let head_path = head.path;
            arena[head_idx] = ArenaSparseNode::Leaf {
                state: ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 },
                key: full_path.slice(head_path.len()..),
                value: value.to_vec(),
            };
            UpsertLeafResult::None
        }
        FindAncestorResult::RevealedLeaf => {
            let head_idx = head.index;
            let head_path = head.path;
            let leaf_key = match &arena[head_idx] {
                ArenaSparseNode::Leaf { key, .. } => *key,
                _ => unreachable!("LeafHead but stack head is not a leaf"),
            };

            let mut leaf_full_path = head_path;
            leaf_full_path.extend(&leaf_key);

            if &leaf_full_path == full_path {
                // Same leaf — update value in place.
                if let ArenaSparseNode::Leaf { value: v, state, .. } = &mut arena[head_idx] {
                    v.clear();
                    v.extend_from_slice(value);
                    *state = ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 };
                }
            } else {
                // Different leaf — split into a branch.
                let remaining_full = full_path.slice(head_path.len()..);
                let new_branch_idx =
                    split_and_insert_leaf(arena, remaining_full, value, &leaf_key, head_idx);

                *root = replace_child_in_parent(arena, stack, *root, head_idx, new_branch_idx);
                stack.last_mut().unwrap().index = new_branch_idx;

                if stack.len() >= 2 {
                    let parent_entry = &stack[stack.len() - 2];
                    let child_nibble = stack.last().unwrap().path.last().unwrap();
                    return UpsertLeafResult::NewBranch {
                        parent_idx: parent_entry.index,
                        child_nibble,
                        parent_path: parent_entry.path,
                    };
                }
            }
            UpsertLeafResult::None
        }
        FindAncestorResult::Diverged => {
            let head_path = head.path;
            let head_idx = head.index;
            let short_key = arena[head_idx].branch_ref().short_key;
            let full_path_from_head = full_path.slice(head_path.len()..);

            let new_branch_idx =
                split_and_insert_leaf(arena, full_path_from_head, value, &short_key, head_idx);

            *root = replace_child_in_parent(arena, stack, *root, head_idx, new_branch_idx);

            stack.last_mut().unwrap().index = new_branch_idx;

            // The new branch replaced the stack head. Its parent is stack[len-2].
            if stack.len() >= 2 {
                let parent_entry = &stack[stack.len() - 2];
                let child_nibble = stack.last().unwrap().path.last().unwrap();
                return UpsertLeafResult::NewBranch {
                    parent_idx: parent_entry.index,
                    child_nibble,
                    parent_path: parent_entry.path,
                };
            }
            UpsertLeafResult::None
        }
        FindAncestorResult::NoChild { child_nibble } => {
            let head_idx = head.index;
            let head_path = head.path;
            let head_branch = arena[head_idx].branch_ref();
            let old_num_leaves = head_branch.state.num_leaves();
            let old_dirty_leaves = head_branch.state.num_dirty_leaves();
            let state_mask = head_branch.state_mask;
            let insert_pos =
                (state_mask.get() & ((1u16 << child_nibble) - 1)).count_ones() as usize;

            let mut head_branch_logical_path = head_path;
            head_branch_logical_path.extend(&head_branch.short_key);
            let leaf_key = full_path.slice(head_branch_logical_path.len() + 1..);
            let new_leaf = arena.insert(ArenaSparseNode::Leaf {
                state: ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 },
                key: leaf_key,
                value: value.to_vec(),
            });

            let branch = arena[head_idx].branch_mut();
            branch.state_mask.set_bit(child_nibble);
            branch.children.insert(insert_pos, ArenaSparseNodeBranchChild::Revealed(new_leaf));
            branch.state = ArenaSparseNodeState::Dirty {
                num_leaves: old_num_leaves + 1,
                num_dirty_leaves: old_dirty_leaves + 1,
            };
            UpsertLeafResult::None
        }
        FindAncestorResult::RevealedSubtrie { .. } => {
            unreachable!("RevealedSubtrie must be handled by caller")
        }
    }
}

/// Result of [`remove_leaf`] indicating whether a proof is needed to complete a branch
/// collapse.
#[derive(Debug)]
enum RemoveLeafResult {
    /// No proof needed — the removal (and any collapse) completed fully.
    None,
    /// The branch collapse requires revealing a blinded sibling. The caller must request a
    /// proof for the given key at the given minimum depth.
    NeedsProof { key: B256, proof_key: B256, min_len: u8 },
}

/// Removes a leaf node from the trie using a pre-computed [`FindAncestorResult`] from
/// [`find_ancestor`].
///
/// Only the `RevealedLeaf` case performs a removal — the leaf must exist and its full path
/// must match `full_path`. All other cases (`Diverged`, `NoChild`) are no-ops since the leaf
/// doesn't exist at that path.
///
/// When removing a leaf from a branch, if the branch is left with only one remaining child,
/// the branch is collapsed: the remaining child absorbs the branch's `short_key` + the child's
/// nibble as a prefix to its own key/`short_key`, and replaces the branch in the parent.
/// If the remaining child is blinded, the collapse cannot proceed and a
/// [`RemoveLeafResult::NeedsProof`] is returned so the caller can request a proof.
///
/// The caller must handle [`FindAncestorResult::Blinded`] and
/// [`FindAncestorResult::RevealedSubtrie`] before calling this function.
fn remove_leaf(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
    root: &mut Index,
    key: B256,
    full_path: &Nibbles,
    find_result: FindAncestorResult,
) -> RemoveLeafResult {
    match find_result {
        FindAncestorResult::Blinded | FindAncestorResult::RevealedSubtrie { .. } => {
            unreachable!("Blinded/RevealedSubtrie must be handled by caller")
        }
        FindAncestorResult::EmptyRoot |
        FindAncestorResult::Diverged |
        FindAncestorResult::NoChild { .. } => RemoveLeafResult::None,
        FindAncestorResult::RevealedLeaf => {
            let head = stack.last().unwrap();
            let head_idx = head.index;
            let head_path = head.path;

            let leaf_key = match &arena[head_idx] {
                ArenaSparseNode::Leaf { key, .. } => *key,
                _ => unreachable!("RevealedLeaf but stack head is not a leaf"),
            };

            let mut leaf_full_path = head_path;
            leaf_full_path.extend(&leaf_key);

            if &leaf_full_path != full_path {
                return RemoveLeafResult::None;
            }

            // Before mutating, check if removing this leaf would leave the parent
            // branch with a single blinded sibling (requiring a proof to collapse).
            if stack.len() >= 2 {
                let parent_entry = &stack[stack.len() - 2];
                let parent_idx = parent_entry.index;
                let child_nibble = head_path.last().unwrap();
                let parent_branch = arena[parent_idx].branch_ref();

                if parent_branch.state_mask.count_bits() == 2 {
                    let dense_idx = child_dense_index(parent_branch.state_mask, child_nibble)
                        .expect("leaf nibble not found in parent state_mask");
                    // With exactly 2 bits set the dense array has indices 0 and 1.
                    let sibling_dense_idx = 1 - dense_idx;
                    if parent_branch.children[sibling_dense_idx].is_blinded() {
                        let parent_path = parent_entry.path;
                        let sibling_nibble =
                            parent_branch.state_mask.iter().find(|&n| n != child_nibble).unwrap();
                        let mut sibling_path = parent_path;
                        sibling_path.extend(&parent_branch.short_key);
                        sibling_path.push_unchecked(sibling_nibble);
                        return RemoveLeafResult::NeedsProof {
                            key,
                            proof_key: nibbles_to_padded_b256(&sibling_path),
                            min_len: (sibling_path.len() as u8).min(64),
                        };
                    }
                }
            }

            // Pop the leaf entry from the stack.
            stack.pop();

            if stack.is_empty() {
                // The leaf is the root — replace with EmptyRoot and push it back onto
                // the stack so subsequent iterations can call find_ancestor normally.
                arena.remove(head_idx);
                *root = arena.insert(ArenaSparseNode::EmptyRoot);
                stack.push(ArenaStackEntry {
                    index: *root,
                    path: head_path,
                    prev_num_leaves: 0,
                    prev_dirty_leaves: 0,
                });
                return RemoveLeafResult::None;
            }

            // The parent must be a branch. Remove the leaf from it.
            let parent_entry = stack.last().unwrap();
            let parent_idx = parent_entry.index;
            let child_nibble = head_path.last().unwrap();

            let parent_branch = arena[parent_idx].branch_ref();
            let dense_idx = child_dense_index(parent_branch.state_mask, child_nibble)
                .expect("leaf nibble not found in parent state_mask");

            let old_num_leaves = parent_branch.state.num_leaves();
            let old_dirty_leaves = parent_branch.state.num_dirty_leaves();

            // Remove the leaf from the arena and from the parent's children.
            arena.remove(head_idx);
            let parent_branch = arena[parent_idx].branch_mut();
            parent_branch.children.remove(dense_idx);
            parent_branch.state_mask.unset_bit(child_nibble);
            parent_branch.state = ArenaSparseNodeState::Dirty {
                num_leaves: old_num_leaves - 1,
                num_dirty_leaves: if old_dirty_leaves > 0 { old_dirty_leaves - 1 } else { 0 },
            };

            // If the branch now has only one child, collapse it. The blinded sibling
            // case was already handled above before any mutations.
            if parent_branch.state_mask.count_bits() == 1 {
                collapse_branch(arena, stack, root, parent_idx);
            }
            RemoveLeafResult::None
        }
    }
}

/// Collapses a branch node that has exactly one remaining revealed child. The branch's
/// `short_key`, the remaining child's nibble, and the child's own key/`short_key` are
/// concatenated to form the child's new key/`short_key`. The child then replaces the branch
/// in the grandparent (or becomes the new root).
///
/// The caller must verify that the remaining child is not blinded before calling this function.
///
/// `branch_idx` must be the index of the branch being collapsed and must be the current stack
/// head. The branch is freed from the arena.
fn collapse_branch(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
    root: &mut Index,
    branch_idx: Index,
) {
    let branch = arena[branch_idx].branch_ref();
    debug_assert_eq!(branch.state_mask.count_bits(), 1, "collapse_branch requires exactly 1 child");
    debug_assert!(
        !branch.children[0].is_blinded(),
        "collapse_branch called with a blinded remaining child"
    );

    let remaining_nibble = branch.state_mask.iter().next().unwrap();
    let branch_short_key = branch.short_key;

    // Build the prefix: branch's short_key + remaining child's nibble.
    let mut prefix = branch_short_key;
    prefix.push_unchecked(remaining_nibble);

    let ArenaSparseNodeBranchChild::Revealed(child_idx) = branch.children[0] else {
        unreachable!()
    };

    // Prepend the prefix to the child's key/short_key and mark dirty.
    match &mut arena[child_idx] {
        ArenaSparseNode::Leaf { key, state, .. } => {
            let mut new_key = prefix;
            new_key.extend(key);
            *key = new_key;
            *state = ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 };
        }
        ArenaSparseNode::Branch(b) => {
            let mut new_short_key = prefix;
            new_short_key.extend(&b.short_key);
            b.short_key = new_short_key;
            b.state = b.state.to_dirty();
        }
        ArenaSparseNode::Subtrie(subtrie) => {
            let ArenaSparseNode::Branch(b) = &mut subtrie.arena[subtrie.root] else {
                unreachable!("subtrie root must be a Branch during collapse_branch")
            };
            let mut new_short_key = prefix;
            new_short_key.extend(&b.short_key);
            b.short_key = new_short_key;
            b.state = b.state.to_dirty();
        }
        _ => unreachable!("remaining child must be Leaf, Branch, or Subtrie"),
    }

    // Replace the branch with the remaining child in the grandparent (or root).
    *root = replace_child_in_parent(arena, stack, *root, branch_idx, child_idx);
    stack.last_mut().unwrap().index = child_idx;

    // Free the collapsed branch.
    arena.remove(branch_idx);
}

/// Reveals a single proof node using a pre-computed [`FindAncestorResult`] from
/// [`find_ancestor`].
///
/// If the result is `Blinded`, the blinded child is replaced with the proof node (converted to
/// an arena node with `Cached` state). If the child is a branch, it is pushed onto the stack.
/// All other cases (already revealed, no child, diverged, leaf head) are no-ops — the proof
/// node is skipped.
fn reveal_node(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
    node: &mut ProofTrieNodeV2,
    find_result: FindAncestorResult,
) {
    let FindAncestorResult::Blinded = find_result else {
        // Already revealed, no child slot, or diverged — skip this proof node.
        return;
    };

    let head = stack.last().unwrap();
    let head_idx = head.index;
    let head_branch = arena[head_idx].branch_ref();

    let mut head_branch_logical_path = head.path;
    head_branch_logical_path.extend(&head_branch.short_key);

    debug_assert_eq!(
        node.path.len(),
        head_branch_logical_path.len() + 1,
        "proof node path {:?} is not a direct child of branch at {:?} (expected depth {})",
        node.path,
        head_branch_logical_path,
        head_branch_logical_path.len() + 1,
    );

    let child_nibble = node.path.get_unchecked(head_branch_logical_path.len());
    let dense_idx = child_dense_index(head_branch.state_mask, child_nibble)
        .expect("Blinded result but child nibble not in state_mask");

    let cached_rlp = match &head_branch.children[dense_idx] {
        ArenaSparseNodeBranchChild::Blinded(rlp) => rlp.clone(),
        ArenaSparseNodeBranchChild::Revealed(_) => return,
    };

    let proof_node = mem::replace(node, ProofTrieNodeV2::empty());
    let mut arena_node = ArenaSparseNode::from(proof_node);

    let state = arena_node.state_mut();
    let num_leaves = state.num_leaves();
    *state = ArenaSparseNodeState::Cached { rlp_node: cached_rlp, num_leaves };

    let child_idx = arena.insert(arena_node);

    arena[head_idx].branch_mut().children[dense_idx] =
        ArenaSparseNodeBranchChild::Revealed(child_idx);

    // Only push branches onto the stack (leaves have no children to descend into).
    if let ArenaSparseNode::Branch(b) = &arena[child_idx] {
        let path = child_path(arena, stack, child_nibble);
        stack.push(ArenaStackEntry {
            index: child_idx,
            path,
            prev_num_leaves: b.state.num_leaves(),
            prev_dirty_leaves: b.state.num_dirty_leaves(),
        });
    }
}

/// A proof request generated during leaf updates when a blinded node is encountered.
#[derive(Debug, Clone)]
struct ArenaRequiredProof {
    /// The key requiring a proof.
    key: B256,
    /// Minimum depth at which proof nodes should be returned.
    min_len: u8,
}

/// An arena-based parallel sparse trie.
///
/// Uses arena allocation for node storage, with direct index-based child pointers. The upper trie
/// and each subtrie maintain their own arenas, enabling parallel mutations of independent subtries.
///
/// ## Upper vs. Lower Trie Placement
///
/// Nodes are split between the upper trie and lower subtries based on their path length (not
/// counting a branch's short key):
///
/// - **Upper trie**: Nodes whose path length is **< 2** nibbles live directly in `upper_arena`.
///   These are branch nodes at paths like `0x` or `0x3`.
/// - **Lower subtries**: Children of upper-trie branches that would have a path length **≥ 2**
///   become the roots of [`ArenaSparseSubtrie`]s, stored as [`ArenaSparseNode::Subtrie`] children
///   of the upper-trie branch.
///
/// A branch's short key can extend its logical reach past the 2-nibble boundary. When this happens
/// the subtrie boundary is "pulled back" to the branch itself, so the entire extension + branch
/// lives inside a single subtrie.
///
/// ### Example 1 — short key crosses the boundary
///
/// ```text
/// Branch at 0x, short_key = 0x123
/// ├── Leaf 0x123a
/// └── Leaf 0x123b
/// ```
///
/// The branch path (`0x`) has length 0, which is < 2, but its short key `0x123` means its
/// children land at path length 4 — well past the boundary. Because the short key crosses
/// the boundary the branch itself becomes a `Subtrie` node in the upper trie. The subtrie's
/// root is the branch at `0x`; everything beneath it (the two leaves) is inside that
/// subtrie's arena.
///
/// ### Example 2 — mixed subtrie depths
///
/// ```text
/// Branch at 0x, short_key = 0x (empty)
/// ├── Branch at 0x1, short_key = 0x (empty)
/// │   ├── Leaf 0x1a
/// │   └── Leaf 0x1b
/// └── Branch at 0x2, short_key = 0x345
///     ├── Leaf 0x2345a
///     └── Leaf 0x2345b
/// ```
///
/// - `0x` is a regular upper-trie branch (path length 0, has child subtries so it stays in the
///   upper trie as a plain branch).
/// - `0x1` is also in the upper trie (path length 1, has child subtries). Its children
///   `0x1a`/`0x1b` are at path length 2, so each becomes its own single-leaf subtrie.
/// - `0x2` has path length 1 and short key `0x345`, which would place its children at path length
///   5. The subtrie is pulled back to `0x2` itself, so the branch and both leaves live in one
///   subtrie rooted at `0x2`.
#[derive(Debug)]
pub struct ArenaParallelSparseTrie {
    /// The arena allocating nodes in the upper trie.
    upper_arena: Arena<ArenaSparseNode>,
    /// The root node of the upper trie.
    root: Index,
    /// Optional tracking of trie updates for database persistence.
    updates: Option<SparseTrieUpdates>,
    /// Reusable stack buffer for trie traversals.
    stack: Vec<ArenaStackEntry>,
    /// Reusable buffer for collecting update actions.
    update_actions: Vec<SparseTrieUpdatesAction>,
    /// Reusable buffer for RLP encoding.
    #[expect(dead_code)]
    rlp_buf: Vec<u8>,
    /// Pool of cleared `ArenaSparseSubtrie`s available for reuse.
    cleared_subtries: Vec<ArenaSparseSubtrie>,
}

impl ArenaParallelSparseTrie {
    /// Returns the arena indexes of all [`ArenaSparseNode::Subtrie`] nodes in the upper arena.
    fn all_subtries(&self) -> SmallVec<[Index; 16]> {
        self.upper_arena
            .iter()
            .filter_map(|(idx, node)| matches!(node, ArenaSparseNode::Subtrie(_)).then_some(idx))
            .collect()
    }

    /// Takes a cleared [`ArenaSparseSubtrie`] from the pool, or creates a new one with the given
    /// root node and its absolute path in the full trie.
    fn take_or_create_subtrie(&mut self, root: ArenaSparseNode) -> Box<ArenaSparseSubtrie> {
        let mut subtrie = if let Some(s) = self.cleared_subtries.pop() {
            debug_assert!(s.arena.is_empty());
            s
        } else {
            ArenaSparseSubtrie {
                arena: Arena::new(),
                root: Index::DANGLING,
                stack: Vec::new(),
                update_actions: Vec::new(),
                required_proofs: Vec::new(),
            }
        };
        subtrie.root = subtrie.arena.insert(root);
        Box::new(subtrie)
    }

    /// Returns `true` if a node at `path` with the given `short_key` should be placed in a
    /// subtrie rather than the upper arena.
    const fn should_be_subtrie(path_len: usize, short_key_len: usize) -> bool {
        path_len >= UPPER_TRIE_MAX_DEPTH || path_len + short_key_len >= UPPER_TRIE_MAX_DEPTH
    }

    /// If the child node at `child_idx` (under `parent_idx` at `child_nibble`) should be a
    /// subtrie based on its depth and short key length, wraps it in
    /// [`ArenaSparseNode::Subtrie`] and updates the parent's child pointer. Also pops the
    /// stack entry if the child was pushed by [`reveal_node`] (since subtrie roots are not
    /// traversed by the upper trie's stack).
    fn maybe_wrap_in_subtrie(
        &mut self,
        parent_idx: Index,
        child_nibble: u8,
        parent_path: Nibbles,
        stack: &mut Vec<ArenaStackEntry>,
    ) {
        let parent = self.upper_arena[parent_idx].branch_ref();
        let child_path_len = parent_path.len() + parent.short_key.len() + 1;
        let Some(child_dense_idx) = child_dense_index(parent.state_mask, child_nibble) else {
            return;
        };
        let ArenaSparseNodeBranchChild::Revealed(child_idx) = parent.children[child_dense_idx]
        else {
            return;
        };

        // Only branch nodes can become subtrie roots.
        let ArenaSparseNode::Branch(child_branch) = &self.upper_arena[child_idx] else {
            return;
        };
        let short_key_len = child_branch.short_key.len();

        if !Self::should_be_subtrie(child_path_len, short_key_len) {
            return;
        }

        // If reveal_node pushed this child onto the stack (because it's a branch), pop it —
        // the upper trie doesn't traverse into subtrie roots.
        if stack.last().is_some_and(|e| e.index == child_idx) {
            stack.pop();
        }

        let arena_node = self.upper_arena.remove(child_idx).unwrap();
        let subtrie = self.take_or_create_subtrie(arena_node);
        let subtrie_idx = self.upper_arena.insert(ArenaSparseNode::Subtrie(subtrie));
        self.upper_arena[parent_idx].branch_mut().children[child_dense_idx] =
            ArenaSparseNodeBranchChild::Revealed(subtrie_idx);
    }

    /// Checks whether a subtrie node at `subtrie_idx` (child of `parent_idx` at
    /// `child_nibble`) should still be a subtrie. If the subtrie's root is no longer a branch,
    /// or if its path + short_key no longer meets the subtrie depth threshold, its root node
    /// is moved into the upper arena and the subtrie is returned to the pool.
    fn maybe_unwrap_subtrie(
        &mut self,
        parent_idx: Index,
        child_nibble: u8,
        parent_path: &Nibbles,
        subtrie_idx: Index,
    ) {
        let ArenaSparseNode::Subtrie(subtrie) = &self.upper_arena[subtrie_idx] else {
            return;
        };

        let subtrie_root = &subtrie.arena[subtrie.root];
        let child_path_len =
            parent_path.len() + self.upper_arena[parent_idx].branch_ref().short_key.len() + 1;

        let should_remain = match subtrie_root {
            ArenaSparseNode::Branch(b) => {
                Self::should_be_subtrie(child_path_len, b.short_key.len())
            }
            _ => false,
        };

        if should_remain {
            return;
        }

        // Extract the subtrie, move its root node into the upper arena, and recycle it.
        let ArenaSparseNode::Subtrie(mut subtrie) = self.upper_arena.remove(subtrie_idx).unwrap()
        else {
            unreachable!()
        };

        let root_node = subtrie.arena.remove(subtrie.root).unwrap();
        let new_child_idx = self.upper_arena.insert(root_node);

        let dense_idx =
            child_dense_index(self.upper_arena[parent_idx].branch_ref().state_mask, child_nibble)
                .expect("child nibble not found in parent state_mask");
        self.upper_arena[parent_idx].branch_mut().children[dense_idx] =
            ArenaSparseNodeBranchChild::Revealed(new_child_idx);

        subtrie.clear();
        self.cleared_subtries.push(*subtrie);
    }
}

impl Default for ArenaParallelSparseTrie {
    fn default() -> Self {
        let mut upper_arena = Arena::new();
        let root = upper_arena.insert(ArenaSparseNode::EmptyRoot);
        Self {
            upper_arena,
            root,
            updates: None,
            stack: Vec::new(),
            update_actions: Vec::new(),
            rlp_buf: Vec::new(),
            cleared_subtries: Vec::new(),
        }
    }
}

impl SparseTrie for ArenaParallelSparseTrie {
    fn set_root(
        &mut self,
        root: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<()> {
        debug_assert!(
            matches!(self.upper_arena[self.root], ArenaSparseNode::EmptyRoot),
            "set_root called on a trie that already has revealed nodes"
        );

        self.set_updates(retain_updates);

        match root {
            TrieNodeV2::EmptyRoot => {
                // Already EmptyRoot, nothing to do.
            }
            TrieNodeV2::Leaf(leaf) => {
                self.upper_arena[self.root] = ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Revealed { num_leaves: 1 },
                    key: leaf.key,
                    value: leaf.value,
                };
            }
            TrieNodeV2::Branch(branch) => {
                let mut children = SmallVec::with_capacity(branch.state_mask.count_bits() as usize);
                for (stack_ptr, _nibble) in branch.state_mask.iter().enumerate() {
                    children
                        .push(ArenaSparseNodeBranchChild::Blinded(branch.stack[stack_ptr].clone()));
                }

                self.upper_arena[self.root] = ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed { num_leaves: 0 },
                    children,
                    state_mask: branch.state_mask,
                    short_key: branch.key,
                    branch_masks: masks.unwrap_or_default(),
                });
            }
            TrieNodeV2::Extension(_) => {
                panic!("set_root does not support Extension nodes; extensions are represented as branches with a short_key")
            }
        }

        Ok(())
    }

    fn set_updates(&mut self, retain_updates: bool) {
        self.updates = retain_updates.then(Default::default);
    }

    fn reserve_nodes(&mut self, _additional: usize) {
        // thunderdome::Arena does not support reserve; no-op.
    }

    fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        if matches!(self.upper_arena[self.root], ArenaSparseNode::EmptyRoot) {
            return Ok(());
        }

        // Sort nodes lexicographically by path.
        nodes.sort_unstable_by(|a, b| a.path.cmp(&b.path));

        let root_num_leaves = self.upper_arena[self.root].num_leaves();

        // Take the stack out to avoid borrow conflicts with `self`.
        let mut stack = mem::take(&mut self.stack);
        stack.clear();
        stack.push(ArenaStackEntry {
            index: self.root,
            path: Nibbles::default(),
            prev_num_leaves: root_num_leaves,
            prev_dirty_leaves: 0,
        });

        // Skip root node if present (set_root handles the root).
        let mut node_idx = if nodes[0].path.is_empty() { 1 } else { 0 };

        while node_idx < nodes.len() {
            let find_result =
                find_ancestor(&mut self.upper_arena, &mut stack, &nodes[node_idx].path);

            match find_result {
                FindAncestorResult::RevealedLeaf => {
                    // Leaf heads have no blinded children — skip.
                    node_idx += 1;
                }
                FindAncestorResult::Blinded => {
                    let head = stack.last().unwrap();
                    let head_idx = head.index;
                    let head_path = head.path;
                    let head_branch = self.upper_arena[head_idx].branch_ref();
                    let child_nibble_offset = head_path.len() + head_branch.short_key.len();
                    let child_nibble = nodes[node_idx].path.get_unchecked(child_nibble_offset);

                    reveal_node(
                        &mut self.upper_arena,
                        &mut stack,
                        &mut nodes[node_idx],
                        FindAncestorResult::Blinded,
                    );
                    node_idx += 1;

                    self.maybe_wrap_in_subtrie(head_idx, child_nibble, head_path, &mut stack);
                }
                FindAncestorResult::RevealedSubtrie { child_idx, child_nibble, .. }
                    if matches!(self.upper_arena[child_idx], ArenaSparseNode::Subtrie(_)) =>
                {
                    let subtrie_start = node_idx;
                    let prefix = child_path(&self.upper_arena, &stack, child_nibble);
                    while node_idx < nodes.len() && nodes[node_idx].path.starts_with(&prefix) {
                        node_idx += 1;
                    }
                    let ArenaSparseNode::Subtrie(mut subtrie) = mem::replace(
                        &mut self.upper_arena[child_idx],
                        ArenaSparseNode::TakenSubtrie,
                    ) else {
                        unreachable!()
                    };
                    let subtrie_nodes = &mut nodes[subtrie_start..node_idx];
                    subtrie.reveal_nodes(subtrie_nodes, prefix)?;
                    self.upper_arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);
                }
                _ => {
                    // Diverged, NoChild, or Revealed non-subtrie — skip this proof node.
                    node_idx += 1;
                }
            }
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.upper_arena, &mut stack);

        // Put the stack back.
        self.stack = stack;

        Ok(())
    }

    fn update_leaf<P: crate::provider::TrieNodeProvider>(
        &mut self,
        _full_path: Nibbles,
        _value: Vec<u8>,
        _provider: P,
    ) -> SparseTrieResult<()> {
        todo!()
    }

    fn remove_leaf<P: crate::provider::TrieNodeProvider>(
        &mut self,
        _full_path: &Nibbles,
        _provider: P,
    ) -> SparseTrieResult<()> {
        todo!()
    }

    fn root(&mut self) -> B256 {
        todo!()
    }

    fn is_root_cached(&self) -> bool {
        matches!(
            &self.upper_arena[self.root],
            ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                state: ArenaSparseNodeState::Cached { .. },
                ..
            }) | ArenaSparseNode::Leaf { state: ArenaSparseNodeState::Cached { .. }, .. }
        )
    }

    fn update_subtrie_hashes(&mut self) {
        todo!()
    }

    fn get_leaf_value(&self, _full_path: &Nibbles) -> Option<&Vec<u8>> {
        todo!()
    }

    fn find_leaf(
        &self,
        _full_path: &Nibbles,
        _expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        todo!()
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.updates.as_ref().map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        match self.updates.take() {
            Some(updates) => {
                self.updates = Some(SparseTrieUpdates::with_capacity(
                    updates.updated_nodes.len(),
                    updates.removed_nodes.len(),
                ));
                updates
            }
            None => SparseTrieUpdates::default(),
        }
    }

    fn wipe(&mut self) {
        self.clear();
        self.updates = self.updates.is_some().then(SparseTrieUpdates::wiped);
    }

    fn clear(&mut self) {
        for idx in self.all_subtries() {
            if let ArenaSparseNode::Subtrie(mut subtrie) = self.upper_arena.remove(idx).unwrap() {
                subtrie.clear();
                self.cleared_subtries.push(*subtrie);
            }
        }
        self.upper_arena.clear();
        self.root = self.upper_arena.insert(ArenaSparseNode::EmptyRoot);
        if let Some(updates) = self.updates.as_mut() {
            updates.clear()
        }
        self.stack.clear();
        self.update_actions.clear();
    }

    fn shrink_nodes_to(&mut self, _size: usize) {
        // Arena does not support shrinking; no-op.
    }

    fn shrink_values_to(&mut self, _size: usize) {
        // No separate value storage; no-op per spec.
    }

    fn size_hint(&self) -> usize {
        self.upper_arena[self.root].num_leaves() as usize
    }

    fn prune(&mut self, _max_depth: usize) -> usize {
        todo!()
    }

    fn update_leaves(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        mut proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()> {
        if updates.is_empty() {
            return Ok(());
        }

        // Drain and sort updates lexicographically by nibbles path.
        let mut sorted: Vec<_> =
            updates.drain().map(|(key, update)| (key, Nibbles::unpack(key), update)).collect();
        sorted.sort_unstable_by(|a, b| a.1.cmp(&b.1));

        let root_num_leaves = self.upper_arena[self.root].num_leaves();
        let mut stack = mem::take(&mut self.stack);
        stack.clear();
        stack.push(ArenaStackEntry {
            index: self.root,
            path: Nibbles::default(),
            prev_num_leaves: root_num_leaves,
            prev_dirty_leaves: 0,
        });

        let mut update_idx = 0;
        while update_idx < sorted.len() {
            let (key, ref full_path, ref update) = sorted[update_idx];

            let find_result = find_ancestor(&mut self.upper_arena, &mut stack, full_path);

            match find_result {
                // Blinded — request a proof regardless of update type.
                FindAncestorResult::Blinded => {
                    let head = stack.last().unwrap();
                    let head_branch = self.upper_arena[head.index].branch_ref();
                    let logical_len = head.path.len() + head_branch.short_key.len();
                    let min_len = (logical_len as u8 + 1).min(64);
                    proof_required_fn(key, min_len);
                    updates.insert(key, LeafUpdate::Touched);
                }
                // Subtrie — forward all consecutive updates under this subtrie's prefix.
                FindAncestorResult::RevealedSubtrie { child_idx, child_nibble, .. } => {
                    let subtrie_start = update_idx;
                    let subtrie_root_path = child_path(&self.upper_arena, &stack, child_nibble);
                    while update_idx < sorted.len() &&
                        sorted[update_idx].1.starts_with(&subtrie_root_path)
                    {
                        update_idx += 1;
                    }

                    let ArenaSparseNode::Subtrie(mut subtrie) = mem::replace(
                        &mut self.upper_arena[child_idx],
                        ArenaSparseNode::TakenSubtrie,
                    ) else {
                        unreachable!()
                    };

                    subtrie.update_leaves(&sorted[subtrie_start..update_idx], subtrie_root_path);

                    for proof in subtrie.required_proofs.drain(..) {
                        proof_required_fn(proof.key, proof.min_len);
                        updates.insert(proof.key, LeafUpdate::Touched);
                    }

                    self.upper_arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);

                    // Check if the subtrie's root still qualifies as a subtrie after updates.
                    let parent_idx = stack.last().unwrap().index;
                    let parent_path = stack.last().unwrap().path;
                    self.maybe_unwrap_subtrie(parent_idx, child_nibble, &parent_path, child_idx);

                    // Don't increment update_idx — already advanced past subtrie updates.
                    continue;
                }
                // EmptyRoot, leaf, diverged branch, or empty child slot — upsert directly.
                find_result @ (FindAncestorResult::EmptyRoot |
                FindAncestorResult::RevealedLeaf |
                FindAncestorResult::Diverged |
                FindAncestorResult::NoChild { .. }) => match update {
                    LeafUpdate::Changed(v) if !v.is_empty() => {
                        let result = upsert_leaf(
                            &mut self.upper_arena,
                            &mut stack,
                            &mut self.root,
                            full_path,
                            v,
                            find_result,
                        );
                        if let UpsertLeafResult::NewBranch {
                            parent_idx,
                            child_nibble,
                            parent_path,
                        } = result
                        {
                            self.maybe_wrap_in_subtrie(
                                parent_idx,
                                child_nibble,
                                parent_path,
                                &mut stack,
                            );
                        }
                    }
                    LeafUpdate::Changed(_) => {
                        let result = remove_leaf(
                            &mut self.upper_arena,
                            &mut stack,
                            &mut self.root,
                            key,
                            full_path,
                            find_result,
                        );
                        if let RemoveLeafResult::NeedsProof { key, proof_key, min_len } = result {
                            proof_required_fn(proof_key, min_len);
                            updates.insert(key, LeafUpdate::Touched);
                        }
                    }
                    LeafUpdate::Touched => {}
                },
            }

            update_idx += 1;
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.upper_arena, &mut stack);
        self.stack = stack;

        Ok(())
    }
}
