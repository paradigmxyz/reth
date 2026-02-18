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
    /// Returns true if the state is [`Self::Dirty`].
    const fn is_dirty(&self) -> bool {
        match self {
            Self::Revealed { .. } | Self::Cached { .. } => false,
            Self::Dirty { .. } => true,
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
    fn add_num_leaves(&mut self, delta: i64) -> u64 {
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
    /// Tree mask and hash mask for database persistence (TrieUpdates).
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

    fn short_key_len(&self) -> usize {
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
                value: leaf.value.to_vec(),
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

/// Returns the index into a densely-packed children array for a given nibble, based on the
/// state mask. Returns `None` if the nibble's bit is not set.
fn child_dense_index(state_mask: TrieMask, nibble: u8) -> Option<usize> {
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
    /// The absolute path of this branch node in the trie (not including its short_key).
    path: Nibbles,
    /// The `num_leaves` value when this branch was pushed onto the stack. Used to compute the
    /// delta to propagate to the parent when popped.
    prev_num_leaves: u64,
    /// The `num_dirty_leaves` value when this branch was pushed onto the stack. Used to compute
    /// the delta to propagate to the parent when popped.
    prev_dirty_leaves: u64,
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

/// If `child_idx` is a branch node, pushes it onto the stack and returns `true`.
/// Otherwise, returns `false`.
///
/// The parent's path and short_key are read from the current top of the stack.
fn maybe_push_branch(
    arena: &Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
    child_idx: Index,
    child_nibble: u8,
) -> bool {
    if let ArenaSparseNode::Branch(b) = &arena[child_idx] {
        let path = child_path(arena, stack, child_nibble);
        stack.push(ArenaStackEntry {
            index: child_idx,
            path,
            prev_num_leaves: b.state.num_leaves(),
            prev_dirty_leaves: b.state.num_dirty_leaves(),
        });
        true
    } else {
        false
    }
}

/// Pops the top entry from the stack and propagates its `num_leaves` and dirty state/count
/// deltas to the new top (its parent). Returns the popped entry.
fn pop_and_propagate(
    arena: &mut Arena<ArenaSparseNode>,
    stack: &mut Vec<ArenaStackEntry>,
) -> Option<ArenaStackEntry> {
    let entry = stack.pop()?;
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

        self.stack.clear();
        let root_node = &self.arena[self.root];
        self.stack.push(ArenaStackEntry {
            index: self.root,
            path: root_path,
            prev_num_leaves: root_node.num_leaves(),
            prev_dirty_leaves: root_node.num_dirty_leaves(),
        });

        for &(key, ref full_path, ref update) in sorted_updates {
            // Pop stack until head is ancestor of full_path.
            while self.stack.len() > 1 && !full_path.starts_with(&self.stack.last().unwrap().path) {
                pop_and_propagate(&mut self.arena, &mut self.stack);
            }

            match update {
                LeafUpdate::Changed(value) if !value.is_empty() => {
                    // Upsert: insert or update a leaf with the given value.
                    self.upsert_leaf(key, full_path, value);
                }
                LeafUpdate::Changed(_) => {
                    // Removal (empty value) — not yet implemented.
                    todo!("Changed removal updates not yet implemented")
                }
                LeafUpdate::Touched => {
                    // Descend from stack head toward target to check accessibility.
                    loop {
                        let head = self.stack.last().unwrap();
                        let head_branch = self.arena[head.index].branch_ref();

                        let mut head_branch_logical_path = head.path;
                        head_branch_logical_path.extend(&head_branch.short_key);
                        debug_assert!(
                            full_path.len() > head_branch_logical_path.len(),
                            "full_path {full_path:?} must be longer than branch logical path {head_branch_logical_path:?}",
                        );

                        if !full_path.starts_with(&head_branch_logical_path) {
                            break; // exclusion — extension arrives at disjoint branch
                        }

                        let child_nibble = full_path.get_unchecked(head_branch_logical_path.len());
                        let Some(dense_idx) =
                            child_dense_index(head_branch.state_mask, child_nibble)
                        else {
                            break; // exclusion — no child at this nibble
                        };

                        match &head_branch.children[dense_idx] {
                            ArenaSparseNodeBranchChild::Blinded(_) => {
                                self.required_proofs.push(ArenaRequiredProof {
                                    key,
                                    min_len: (head_branch_logical_path.len() as u8 + 1).min(64),
                                });
                                break;
                            }
                            ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                                let child_idx = *child_idx;
                                if !maybe_push_branch(
                                    &self.arena,
                                    &mut self.stack,
                                    child_idx,
                                    child_nibble,
                                ) {
                                    // Child is a leaf or empty — accessible or exclusion.
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.arena, &mut self.stack);
    }

    /// Performs a leaf upsert within this subtrie. Descends from the current stack head
    /// toward `full_path`, inserting or updating the leaf with `value`.
    ///
    /// Handles three cases:
    /// 1. The target nibble has no child — inserts a new leaf.
    /// 2. The target nibble has a leaf — updates its value or splits on key divergence.
    /// 3. The target nibble has a branch/leaf whose short_key partially matches — splits by
    ///    creating a new intermediate branch at the divergence point.
    ///
    /// The stack must have at least one entry (the ancestor branch) when called. All
    /// ancestor branches on the stack are marked dirty.
    fn upsert_leaf(&mut self, key: B256, full_path: &Nibbles, value: &[u8]) {
        // Descend from the stack head toward the target leaf.
        loop {
            let head = self.stack.last().unwrap();
            let head_idx = head.index;
            let head_path = head.path;
            let head_branch = self.arena[head_idx].branch_ref();

            let short_key = head_branch.short_key.clone();
            let state_mask = head_branch.state_mask;
            let old_num_leaves = head_branch.state.num_leaves();
            let old_dirty_leaves = head_branch.state.num_dirty_leaves();

            let mut head_branch_logical_path = head_path;
            head_branch_logical_path.extend(&short_key);

            debug_assert!(
                full_path.len() > head_branch_logical_path.len(),
                "full_path {full_path:?} must be longer than branch prefix {head_branch_logical_path:?}",
            );

            // Check if full_path diverges within the branch's short_key. If so, we need
            // to split the short_key by creating a new intermediate branch.
            if !full_path.starts_with(&head_branch_logical_path) {
                let full_path_from_head = full_path.slice(head_path.len()..);
                let common_len = full_path_from_head.common_prefix_length(&short_key);

                // Shorten the existing branch's short_key to everything after the split.
                self.arena[head_idx].branch_mut().short_key = short_key.slice(common_len + 1..);

                let old_child_nibble = short_key.get_unchecked(common_len);
                let old_child_num_leaves = self.arena[head_idx].num_leaves();
                let new_branch_idx = self.split_and_insert_leaf(
                    full_path_from_head,
                    common_len,
                    value,
                    old_child_nibble,
                    head_idx,
                    old_child_num_leaves,
                    old_dirty_leaves,
                    short_key.slice(..common_len),
                );

                self.replace_child_in_parent(head_idx, new_branch_idx);

                let stack_entry = self.stack.last_mut().unwrap();
                stack_entry.index = new_branch_idx;

                return;
            }

            let child_nibble = full_path.get_unchecked(head_branch_logical_path.len());

            match child_dense_index(state_mask, child_nibble) {
                None => {
                    // No child at this nibble — insert a new leaf.
                    let leaf_key: Nibbles = full_path.slice(head_branch_logical_path.len() + 1..);
                    let new_leaf = self.arena.insert(ArenaSparseNode::Leaf {
                        state: ArenaSparseNodeState::Dirty { num_leaves: 1, num_dirty_leaves: 1 },
                        key: leaf_key,
                        value: value.to_vec(),
                    });

                    let insert_pos =
                        (state_mask.get() & ((1u16 << child_nibble) - 1)).count_ones() as usize;
                    let branch = self.arena[head_idx].branch_mut();
                    branch.state_mask.set_bit(child_nibble);
                    branch
                        .children
                        .insert(insert_pos, ArenaSparseNodeBranchChild::Revealed(new_leaf));
                    branch.state = ArenaSparseNodeState::Dirty {
                        num_leaves: old_num_leaves + 1,
                        num_dirty_leaves: old_dirty_leaves + 1,
                    };

                    return;
                }
                Some(dense_idx) => {
                    // Extract child info without holding borrow on arena.
                    let child_idx = match &self.arena[head_idx].branch_ref().children[dense_idx] {
                        ArenaSparseNodeBranchChild::Blinded(_) => {
                            self.required_proofs.push(ArenaRequiredProof {
                                key,
                                min_len: (head_branch_logical_path.len() as u8 + 1).min(64),
                            });
                            return;
                        }
                        ArenaSparseNodeBranchChild::Revealed(idx) => *idx,
                    };

                    match &self.arena[child_idx] {
                        ArenaSparseNode::Leaf { key: leaf_key, .. } => {
                            let leaf_key = leaf_key.clone();

                            // Build the leaf's full path to compare.
                            let mut leaf_full_path = head_branch_logical_path;
                            leaf_full_path.push_unchecked(child_nibble);
                            leaf_full_path.extend(&leaf_key);

                            if &leaf_full_path == full_path {
                                // Same leaf — update value in place.
                                let leaf = &mut self.arena[child_idx];
                                if let ArenaSparseNode::Leaf { value: v, state, .. } = leaf {
                                    v.clear();
                                    v.extend_from_slice(value);
                                    *state = ArenaSparseNodeState::Dirty {
                                        num_leaves: 1,
                                        num_dirty_leaves: 1,
                                    };
                                }
                                self.arena[head_idx].branch_mut().state =
                                    ArenaSparseNodeState::Dirty {
                                        num_leaves: old_num_leaves,
                                        num_dirty_leaves: old_dirty_leaves + 1,
                                    };
                                return;
                            }

                            // Different leaf — find divergence point and split.
                            let child_path_start = head_branch_logical_path.len() + 1;
                            let remaining_full = full_path.slice(child_path_start..);
                            let common_prefix_len = remaining_full.common_prefix_length(&leaf_key);

                            let old_leaf_nibble = leaf_key.get_unchecked(common_prefix_len);
                            let old_leaf_suffix: Nibbles = leaf_key.slice(common_prefix_len + 1..);

                            if let ArenaSparseNode::Leaf { key: k, state, .. } =
                                &mut self.arena[child_idx]
                            {
                                *k = old_leaf_suffix;
                                *state = ArenaSparseNodeState::Dirty {
                                    num_leaves: 1,
                                    num_dirty_leaves: 1,
                                };
                            } else {
                                unreachable!()
                            }

                            let new_branch_idx = self.split_and_insert_leaf(
                                remaining_full,
                                common_prefix_len,
                                value,
                                old_leaf_nibble,
                                child_idx,
                                1,
                                0,
                                remaining_full.slice(..common_prefix_len),
                            );

                            let branch = self.arena[head_idx].branch_mut();
                            branch.children[dense_idx] =
                                ArenaSparseNodeBranchChild::Revealed(new_branch_idx);
                            branch.state = ArenaSparseNodeState::Dirty {
                                num_leaves: old_num_leaves + 1,
                                num_dirty_leaves: old_dirty_leaves + 1,
                            };

                            return;
                        }
                        ArenaSparseNode::Branch(_) => {
                            let _pushed = maybe_push_branch(
                                &self.arena,
                                &mut self.stack,
                                child_idx,
                                child_nibble,
                            );
                            debug_assert!(_pushed);
                            continue;
                        }
                        _ => unreachable!("unexpected node type in subtrie during upsert"),
                    }
                }
            }
        }
    }

    /// Creates a new leaf and a new branch that splits an existing child from the new leaf at
    /// a divergence point. Returns the index of the new branch.
    ///
    /// `new_leaf_path` is the full remaining path for the new leaf (relative to the split
    /// point's parent). `diverge_len` is the common prefix length where the paths diverge.
    /// `old_child_nibble` and `old_child_idx` identify the existing child at the split.
    /// `old_child_num_leaves` and `old_child_dirty_leaves` are the existing child's leaf counts.
    fn split_and_insert_leaf(
        &mut self,
        new_leaf_path: Nibbles,
        diverge_len: usize,
        value: &[u8],
        old_child_nibble: u8,
        old_child_idx: Index,
        old_child_num_leaves: u64,
        old_child_dirty_leaves: u64,
        short_key: Nibbles,
    ) -> Index {
        let new_leaf_nibble = new_leaf_path.get_unchecked(diverge_len);
        debug_assert_ne!(old_child_nibble, new_leaf_nibble);

        let new_leaf_idx = self.arena.insert(ArenaSparseNode::Leaf {
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

        self.arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
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
    /// stack entry. If the stack has only one entry, `old_idx` is the subtrie root and
    /// `self.root` is updated instead.
    ///
    /// `old_idx` must be on the stack (it is the current stack head), so its nibble within
    /// the parent can be determined from the last nibble of its path.
    fn replace_child_in_parent(&mut self, old_idx: Index, new_idx: Index) {
        if self.stack.len() <= 1 {
            self.root = new_idx;
            return;
        }

        let child_path = &self.stack.last().unwrap().path;
        let child_nibble = child_path.last().unwrap();

        let parent_entry = &self.stack[self.stack.len() - 2];
        let parent = self.arena[parent_entry.index].branch_mut();
        let dense_idx = child_dense_index(parent.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");
        debug_assert!(
            matches!(parent.children[dense_idx], ArenaSparseNodeBranchChild::Revealed(idx) if idx == old_idx),
            "parent child at nibble {child_nibble} does not match old_idx",
        );
        parent.children[dense_idx] = ArenaSparseNodeBranchChild::Revealed(new_idx);
    }

    /// Reveals nodes inside this subtrie. Uses the same walk-down algorithm as the upper
    /// trie but operates on the subtrie's own arena.
    ///
    /// `root_path` is the absolute path of the subtrie's root node in the full trie (not
    /// including the root's short_key).
    fn reveal_nodes(
        &mut self,
        nodes: &mut [ProofTrieNodeV2],
        root_path: Nibbles,
    ) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        let root_num_leaves = self.arena[self.root].num_leaves();

        self.stack.clear();
        self.stack.push(ArenaStackEntry {
            index: self.root,
            path: root_path,
            prev_num_leaves: root_num_leaves,
            prev_dirty_leaves: 0,
        });

        for node in nodes.iter_mut() {
            let target_path = &node.path;

            // Pop stack until head is ancestor of target_path.
            while let Some(head) = self.stack.last() &&
                !target_path.starts_with(&head.path)
            {
                pop_and_propagate(&mut self.arena, &mut self.stack);
            }

            // Descend to find the blinded child to replace.
            loop {
                let head = self.stack.last().unwrap();
                let head_path = head.path;
                let head_idx = head.index;
                let head_branch = self.arena[head_idx].branch_ref();

                let child_nibble =
                    target_path.get_unchecked(head_path.len() + head_branch.short_key.len());
                let Some(dense_idx) = child_dense_index(head_branch.state_mask, child_nibble)
                else {
                    break;
                };

                match &head_branch.children[dense_idx] {
                    ArenaSparseNodeBranchChild::Blinded(rlp) => {
                        let cached_rlp = rlp.clone();
                        let proof_node = mem::replace(node, ProofTrieNodeV2::empty());
                        let mut arena_node = ArenaSparseNode::from(proof_node);

                        // Set the child's state to Cached using the parent's RlpNode.
                        let state = arena_node.state_mut();
                        let num_leaves = state.num_leaves();
                        *state = ArenaSparseNodeState::Cached { rlp_node: cached_rlp, num_leaves };

                        let child_idx = self.arena.insert(arena_node);

                        self.arena[head_idx].branch_mut().children[dense_idx] =
                            ArenaSparseNodeBranchChild::Revealed(child_idx);

                        maybe_push_branch(&self.arena, &mut self.stack, child_idx, child_nibble);
                        break;
                    }
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                        let child_idx = *child_idx;
                        if !maybe_push_branch(&self.arena, &mut self.stack, child_idx, child_nibble)
                        {
                            break;
                        }
                    }
                }
            }
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.arena, &mut self.stack);

        Ok(())
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
    fn should_be_subtrie(path_len: usize, short_key_len: usize) -> bool {
        path_len >= UPPER_TRIE_MAX_DEPTH || path_len + short_key_len >= UPPER_TRIE_MAX_DEPTH
    }

    /// Reveals a child node of a branch in the upper arena, replacing its `Blinded` entry.
    ///
    /// If the child should be a subtrie, it is wrapped in [`ArenaSparseNode::Subtrie`].
    /// Otherwise, it is inserted directly into the upper arena. If the child is a branch, it is
    /// pushed onto the stack for further descent.
    fn reveal_upper_trie_child(
        &mut self,
        parent_idx: Index,
        child_nibble: u8,
        parent_path: Nibbles,
        proof_node: ProofTrieNodeV2,
        stack: &mut Vec<ArenaStackEntry>,
    ) -> SparseTrieResult<()> {
        // Take the cached RlpNode from the parent's Blinded child before replacing it.
        let parent = self.upper_arena[parent_idx].branch_ref();
        let Some(child_dense_idx) = child_dense_index(parent.state_mask, child_nibble) else {
            // If the child bit is no longer set then this portion of the trie has been removed via
            // `update_leaves`, the revealed node is therefore out-of-date and should be dropped.
            return Ok(());
        };
        let cached_rlp = match &parent.children[child_dense_idx] {
            ArenaSparseNodeBranchChild::Blinded(rlp) => rlp.clone(),
            ArenaSparseNodeBranchChild::Revealed(_) => {
                // If the child is already revealed we don't re-reveal it, in case it's been
                // updated since.
                return Ok(())
            }
        };

        let mut arena_node = ArenaSparseNode::from(proof_node);

        // Set the child's state to Cached using the parent's RlpNode.
        let state = arena_node.state_mut();
        let num_leaves = state.num_leaves();
        *state = ArenaSparseNodeState::Cached { rlp_node: cached_rlp, num_leaves };

        let short_key_len = arena_node.short_key_len();
        let child_path_len = parent_path.len() + parent.short_key.len() + 1;

        let child_idx = if Self::should_be_subtrie(child_path_len, short_key_len) {
            let subtrie = self.take_or_create_subtrie(arena_node);
            self.upper_arena.insert(ArenaSparseNode::Subtrie(subtrie))
        } else {
            self.upper_arena.insert(arena_node)
        };

        self.upper_arena[parent_idx].branch_mut().children[child_dense_idx] =
            ArenaSparseNodeBranchChild::Revealed(child_idx);

        // If the child is an upper-trie branch, push it onto the stack for further descent.
        maybe_push_branch(&self.upper_arena, stack, child_idx, child_nibble);

        Ok(())
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
                    value: leaf.value.to_vec(),
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

        let mut node_idx = 0;

        // Skip root node if present (set_root handles the root).
        if nodes[0].path.is_empty() {
            node_idx = 1;
        }

        while node_idx < nodes.len() {
            let target_path = &nodes[node_idx].path;

            // Pop stack until head is ancestor of target_path.
            while let Some(head) = stack.last() &&
                !target_path.starts_with(&head.path)
            {
                pop_and_propagate(&mut self.upper_arena, &mut stack);
            }

            // Descend from the current stack head toward the target. We may need to traverse
            // through already-revealed branch nodes.
            loop {
                let head = stack.last().unwrap();
                let head_path = head.path;
                let head_idx = head.index;
                let head_branch = self.upper_arena[head_idx].branch_ref();

                debug_assert!(
                    target_path.starts_with(&head_path),
                    "target {target_path:?} is not under stack head {head_path:?}",
                );

                let child_nibble =
                    target_path.get_unchecked(head_path.len() + head_branch.short_key.len());

                let Some(dense_idx) = child_dense_index(head_branch.state_mask, child_nibble)
                else {
                    node_idx += 1;
                    break;
                };

                match &head_branch.children[dense_idx] {
                    ArenaSparseNodeBranchChild::Blinded(_) => {
                        let proof_node =
                            mem::replace(&mut nodes[node_idx], ProofTrieNodeV2::empty());
                        node_idx += 1;

                        self.reveal_upper_trie_child(
                            head_idx,
                            child_nibble,
                            head_path,
                            proof_node,
                            &mut stack,
                        )?;
                        break;
                    }
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                        let child_idx = *child_idx;
                        match &self.upper_arena[child_idx] {
                            ArenaSparseNode::Branch(_) => {
                                debug_assert!(maybe_push_branch(
                                    &self.upper_arena,
                                    &mut stack,
                                    child_idx,
                                    child_nibble,
                                ));
                            }
                            ArenaSparseNode::Subtrie(_) => {
                                let subtrie_start = node_idx;
                                let prefix = child_path(&self.upper_arena, &stack, child_nibble);
                                while node_idx < nodes.len() &&
                                    nodes[node_idx].path.starts_with(&prefix)
                                {
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
                                break;
                            }
                            _ => {
                                node_idx += 1;
                                break;
                            }
                        }
                    }
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
        'outer: while update_idx < sorted.len() {
            let (key, ref full_path, ref update) = sorted[update_idx];

            // Pop stack until head is ancestor of full_path.
            while stack.len() > 1 && !full_path.starts_with(&stack.last().unwrap().path) {
                pop_and_propagate(&mut self.upper_arena, &mut stack);
            }

            match update {
                LeafUpdate::Changed(_) => todo!("Changed updates not yet implemented"),
                LeafUpdate::Touched => {
                    // Descend from stack head toward target to check accessibility.
                    let proof_target = 'descent: {
                        loop {
                            let head = stack.last().unwrap();
                            let head_branch = self.upper_arena[head.index].branch_ref();

                            // Check if full_path diverges within the branch's short_key.
                            let mut head_branch_logical_path = head.path;
                            head_branch_logical_path.extend(&head_branch.short_key);
                            debug_assert!(
                                full_path.len() > head_branch_logical_path.len(),
                                "full_path {full_path:?} must be longer than branch logical path {head_branch_logical_path:?}"
                            );

                            if !full_path.starts_with(&head_branch_logical_path) {
                                break 'descent None; // exclusion
                            }

                            let child_nibble =
                                full_path.get_unchecked(head_branch_logical_path.len());
                            let Some(dense_idx) =
                                child_dense_index(head_branch.state_mask, child_nibble)
                            else {
                                break 'descent None; // exclusion — no child at this nibble
                            };

                            match &head_branch.children[dense_idx] {
                                ArenaSparseNodeBranchChild::Blinded(_) => {
                                    let min_len =
                                        (head_branch_logical_path.len() as u8 + 1).min(64);
                                    break 'descent Some((key, min_len));
                                }
                                ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                                    let child_idx = *child_idx;
                                    match &self.upper_arena[child_idx] {
                                        ArenaSparseNode::Branch(_) => {
                                            let _pushed = maybe_push_branch(
                                                &self.upper_arena,
                                                &mut stack,
                                                child_idx,
                                                child_nibble,
                                            );
                                            debug_assert!(_pushed);
                                            continue;
                                        }
                                        ArenaSparseNode::Leaf { .. } => {
                                            break 'descent None; // accessible or exclusion
                                        }
                                        ArenaSparseNode::Subtrie(_) => {
                                            // Collect all updates belonging to this subtrie.
                                            let subtrie_start = update_idx;
                                            let mut subtrie_root_path = head_branch_logical_path;
                                            subtrie_root_path.push_unchecked(child_nibble);
                                            while update_idx < sorted.len() &&
                                                sorted[update_idx]
                                                    .1
                                                    .starts_with(&subtrie_root_path)
                                            {
                                                update_idx += 1;
                                            }

                                            // Take the subtrie, apply updates, drain proofs,
                                            // put it back.
                                            let ArenaSparseNode::Subtrie(mut subtrie) =
                                                mem::replace(
                                                    &mut self.upper_arena[child_idx],
                                                    ArenaSparseNode::TakenSubtrie,
                                                )
                                            else {
                                                unreachable!()
                                            };

                                            subtrie.update_leaves(
                                                &sorted[subtrie_start..update_idx],
                                                subtrie_root_path,
                                            );

                                            for proof in subtrie.required_proofs.drain(..) {
                                                proof_required_fn(proof.key, proof.min_len);
                                                updates.insert(proof.key, LeafUpdate::Touched);
                                            }

                                            self.upper_arena[child_idx] =
                                                ArenaSparseNode::Subtrie(subtrie);

                                            // update_idx already advanced past all subtrie
                                            // entries; continue the outer loop directly.
                                            continue 'outer;
                                        }
                                        ArenaSparseNode::EmptyRoot |
                                        ArenaSparseNode::TakenSubtrie => {
                                            unreachable!()
                                        }
                                    }
                                }
                            }
                        }
                    };

                    if let Some((target_key, min_len)) = proof_target {
                        proof_required_fn(target_key, min_len);
                        updates.insert(key, LeafUpdate::Touched);
                    }

                    update_idx += 1;
                }
            }
        }

        // Drain remaining stack entries, propagating num_leaves deltas.
        drain_stack(&mut self.upper_arena, &mut stack);
        self.stack = stack;

        Ok(())
    }
}
