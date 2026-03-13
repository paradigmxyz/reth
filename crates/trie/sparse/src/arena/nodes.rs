use super::{ArenaSparseSubtrie, Index, NodeArena};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::{keccak256, B256};
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};
use slotmap::Key as _;
use smallvec::SmallVec;

/// Tracks whether a node's RLP encoding is cached or needs recomputation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArenaSparseNodeState {
    /// The node has been revealed but its RLP encoding is not cached.
    Revealed,
    /// The node has a cached RLP encoding that is still valid.
    Cached {
        /// The cached RLP-encoded representation of the node.
        rlp_node: RlpNode,
    },
    /// The node has been modified and its RLP encoding needs recomputation.
    Dirty,
}

impl ArenaSparseNodeState {
    /// Returns the [`RlpNode`] cached on the state, if there is one.
    pub(super) const fn cached_rlp_node(&self) -> Option<&RlpNode> {
        match self {
            Self::Cached { rlp_node, .. } => Some(rlp_node),
            _ => None,
        }
    }

    /// Returns `true` if this is the [`Self::Dirty`] variant.
    pub(super) const fn is_dirty(&self) -> bool {
        matches!(self, Self::Dirty)
    }
}

/// Creates the null index sentinel used for empty slots in the branch children array.
///
/// `DefaultKey::null()` is not const, so we call it at runtime.
fn null_index() -> Index {
    Index::null()
}

/// Creates a `[Index; 16]` array filled with null indices.
pub(super) fn null_children() -> [Index; 16] {
    [null_index(); 16]
}

/// Hot branch data — only fields needed for traversal (seek/next).
///
/// Layout uses `#[repr(C)]` to ensure `state_mask`, `revealed_mask`, and `short_key` are
/// placed **before** the 128-byte `children` array. This keeps the seek-hot fields (masks +
/// short_key) in the same cache line as the enum discriminant, so a single node fetch during
/// `seek` only needs 1 guaranteed L3 miss instead of 2.
#[derive(Debug, Clone)]
#[repr(C)]
pub(super) struct ArenaBranchHot {
    /// Bitmask indicating which of the 16 child slots are occupied.
    pub(super) state_mask: TrieMask,
    /// Bitmask indicating which occupied children are revealed (present in the arena).
    /// Invariant: `revealed_mask` is always a subset of `state_mask`.
    pub(super) revealed_mask: TrieMask,
    /// The short key (extension key) for this branch.
    pub(super) short_key: Nibbles,
    /// Direct nibble-indexed child array. `children[nibble]` is valid when
    /// `revealed_mask.is_bit_set(nibble)` is true. Unused slots contain null indices.
    pub(super) children: [Index; 16],
}

impl ArenaBranchHot {
    /// Returns the revealed child index at `nibble`, or `None` if the child is not revealed.
    pub(super) fn revealed_child(&self, nibble: u8) -> Option<Index> {
        if self.revealed_mask.is_bit_set(nibble) {
            Some(self.children[nibble as usize])
        } else {
            None
        }
    }

    /// Returns `true` if the child at `nibble` is blinded (occupied but not revealed).
    pub(super) fn is_child_blinded(&self, nibble: u8) -> bool {
        self.state_mask.is_bit_set(nibble) && !self.revealed_mask.is_bit_set(nibble)
    }

    /// Returns the number of occupied children.
    pub(super) fn count_children(&self) -> u32 {
        self.state_mask.count_bits() as u32
    }

    /// Returns the sibling nibble in a branch with exactly 2 children.
    ///
    /// # Panics
    ///
    /// Panics (debug) if the branch does not have exactly 2 children.
    pub(super) fn sibling_nibble(&self, nibble: u8) -> u8 {
        debug_assert_eq!(self.count_children(), 2, "sibling_nibble requires exactly 2 children");
        self.state_mask.iter().find(|&n| n != nibble).expect("branch has two children")
    }

    /// Returns `true` if the sibling of `nibble` is blinded, in a branch with exactly 2
    /// children.
    pub(super) fn is_sibling_blinded(&self, nibble: u8) -> bool {
        let sib = self.sibling_nibble(nibble);
        self.is_child_blinded(sib)
    }

    /// Iterates over `(nibble, child_index)` pairs for revealed children in nibble order.
    pub(super) fn revealed_iter(&self) -> impl Iterator<Item = (u8, Index)> + '_ {
        self.revealed_mask.iter().map(|nibble| (nibble, self.children[nibble as usize]))
    }

    /// Sets a child as revealed at `nibble`.
    pub(super) fn set_revealed_child(&mut self, nibble: u8, child_idx: Index) {
        self.state_mask.set_bit(nibble);
        self.revealed_mask.set_bit(nibble);
        self.children[nibble as usize] = child_idx;
    }

    /// Removes a child at `nibble`, clearing it from both masks.
    pub(super) fn remove_child(&mut self, nibble: u8) {
        self.state_mask.unset_bit(nibble);
        self.revealed_mask.unset_bit(nibble);
        self.children[nibble as usize] = null_index();
    }
}

/// Cold branch data — state, masks, and blinded children. Only accessed during hashing,
/// encoding, updates, and pruning.
#[derive(Debug, Clone)]
pub(super) struct ArenaBranchCold {
    /// Cached or dirty state of this node.
    pub(super) state: ArenaSparseNodeState,
    /// Tree mask and hash mask for database persistence (`TrieUpdates`).
    pub(super) branch_masks: BranchNodeMasks,
    /// Blinded children's RLP encodings, stored densely in nibble order.
    /// The i-th entry corresponds to the i-th set bit of
    /// `state_mask & !revealed_mask` (the "blinded mask").
    pub(super) blinded: SmallVec<[RlpNode; 4]>,
}

/// Cold data for nodes that have state (branches and leaves).
#[derive(Debug, Clone)]
pub(super) enum ArenaSparseNodeCold {
    /// Cold data for a branch node.
    Branch(ArenaBranchCold),
    /// Cold data for a leaf node.
    Leaf {
        /// Cached or dirty state of this leaf.
        state: ArenaSparseNodeState,
        /// The RLP-encoded leaf value.
        value: Vec<u8>,
    },
}

impl ArenaSparseNodeCold {
    /// Returns a reference to the state.
    pub(super) fn state(&self) -> &ArenaSparseNodeState {
        match self {
            Self::Branch(b) => &b.state,
            Self::Leaf { state, .. } => state,
        }
    }

    /// Returns a mutable reference to the state.
    pub(super) fn state_mut(&mut self) -> &mut ArenaSparseNodeState {
        match self {
            Self::Branch(b) => &mut b.state,
            Self::Leaf { state, .. } => state,
        }
    }

    /// Returns a reference to the branch cold data.
    ///
    /// # Panics
    ///
    /// Panics if this is not a `Branch`.
    pub(super) fn branch(&self) -> &ArenaBranchCold {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch() called on non-Branch cold node"),
        }
    }

    /// Returns a mutable reference to the branch cold data.
    ///
    /// # Panics
    ///
    /// Panics if this is not a `Branch`.
    pub(super) fn branch_mut(&mut self) -> &mut ArenaBranchCold {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch_mut() called on non-Branch cold node"),
        }
    }
}

/// A node in the arena-based sparse trie (hot data only).
///
/// Contains only fields needed for trie traversal. Cold data (state, RLP encoding,
/// leaf values, branch masks, blinded children) is stored separately in
/// [`ArenaSparseNodeCold`] via a `SecondaryMap`.
#[derive(Debug, Clone)]
pub(super) enum ArenaSparseNode {
    /// Indicates a trie with no nodes.
    EmptyRoot,
    /// A branch node with up to 16 children.
    Branch(ArenaBranchHot),
    /// A leaf node containing a key suffix.
    Leaf {
        /// The remaining key suffix for this leaf.
        key: Nibbles,
    },
    /// A subtrie that can be taken for parallel processing.
    Subtrie(Box<ArenaSparseSubtrie>),
    /// Placeholder for a subtrie that has been temporarily taken for parallel operations.
    TakenSubtrie,
}

impl ArenaSparseNode {
    /// Returns the short key of the branch or leaf, or None.
    pub(super) const fn short_key(&self) -> Option<&Nibbles> {
        match self {
            Self::Branch(b) => Some(&b.short_key),
            Self::Leaf { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Returns a reference to the hot branch data.
    ///
    /// # Panics
    ///
    /// Panics if this is not a `Branch` node.
    pub(super) fn branch_ref(&self) -> &ArenaBranchHot {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch_ref called on non-Branch node {self:?}"),
        }
    }

    /// Returns a mutable reference to the hot branch data.
    ///
    /// # Panics
    ///
    /// Panics if this is not a `Branch` node.
    pub(super) fn branch_mut(&mut self) -> &mut ArenaBranchHot {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch_mut called on non-Branch node {self:?}"),
        }
    }

    /// Returns a reference to the subtrie if this is a `Subtrie` node, or `None`.
    #[cfg(debug_assertions)]
    pub(super) const fn as_subtrie(&self) -> Option<&ArenaSparseSubtrie> {
        match self {
            Self::Subtrie(s) => Some(s),
            _ => None,
        }
    }
}

/// Extension methods on [`NodeArena`] that require both hot and cold data.
impl NodeArena {
    /// Returns the state of a node, or `None` for EmptyRoot/TakenSubtrie.
    pub(super) fn state_ref(&self, idx: Index) -> Option<&ArenaSparseNodeState> {
        match &self[idx] {
            ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. } => {
                Some(self.cold(idx).state())
            }
            ArenaSparseNode::Subtrie(s) => s.arena.state_ref(s.root),
            _ => None,
        }
    }

    /// Returns `true` if this node's RLP encoding is cached.
    pub(super) fn is_cached(&self, idx: Index) -> bool {
        self.state_ref(idx)
            .is_some_and(|s| matches!(s, ArenaSparseNodeState::Cached { .. }))
    }

    /// Returns the branch data if this node (or its subtrie root) is a branch, or `None`.
    pub(super) fn as_branch(&self, idx: Index) -> Option<&ArenaBranchHot> {
        match &self[idx] {
            ArenaSparseNode::Branch(b) => Some(b),
            ArenaSparseNode::Subtrie(s) => s.arena.as_branch(s.root),
            _ => None,
        }
    }

    /// Returns the cold branch data if this node (or its subtrie root) is a branch, or `None`.
    pub(super) fn as_branch_cold(&self, idx: Index) -> Option<&ArenaBranchCold> {
        match &self[idx] {
            ArenaSparseNode::Branch(_) => Some(self.cold(idx).branch()),
            ArenaSparseNode::Subtrie(s) => s.arena.as_branch_cold(s.root),
            _ => None,
        }
    }

    /// Returns `true` if this node should contribute a set bit in its parent's `hash_mask`.
    pub(super) fn hash_mask_bit(&self, idx: Index) -> bool {
        self.as_branch(idx).is_some_and(|b| {
            b.short_key.is_empty() &&
                self.as_branch_cold(idx)
                    .expect("branch has cold data")
                    .state
                    .cached_rlp_node()
                    .expect("branch's RlpNode must be cached")
                    .is_hash()
        })
    }

    /// Returns `true` if this node should contribute a set bit in its parent's `tree_mask`.
    pub(super) fn tree_mask_bit(&self, idx: Index) -> bool {
        self.as_branch_cold(idx).is_some_and(|b| !b.branch_masks.is_empty())
    }

    /// Returns the cached hash of this node. Panics if the node's state is not `Cached`.
    pub(super) fn cached_hash(&self, idx: Index) -> B256 {
        let rlp_node = match &self[idx] {
            ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. } => self
                .cold(idx)
                .state()
                .cached_rlp_node()
                .expect("cached_hash called on non-Cached node"),
            ArenaSparseNode::Subtrie(s) => return s.arena.cached_hash(s.root),
            _ => panic!("cached_hash called on EmptyRoot/TakenSubtrie"),
        };
        rlp_node.as_hash().unwrap_or_else(|| keccak256(rlp_node.as_slice()))
    }

    /// Returns a [`BranchNodeCompact`] from this branch's masks and children hashes.
    pub(super) fn branch_node_compact(&self, idx: Index) -> BranchNodeCompact {
        let hot = self[idx].branch_ref();
        let cold = self.cold(idx).branch();
        let mut hashes = Vec::new();
        for nibble in hot.state_mask.iter() {
            if cold.branch_masks.hash_mask.is_bit_set(nibble) {
                let hash = if hot.revealed_mask.is_bit_set(nibble) {
                    self.cached_hash(hot.children[nibble as usize])
                } else {
                    let blinded_idx = blinded_dense_index(hot, nibble);
                    cold.blinded[blinded_idx]
                        .as_hash()
                        .expect("blinded child must be a hash")
                };
                hashes.push(hash);
            }
        }
        BranchNodeCompact::new(
            hot.state_mask,
            cold.branch_masks.tree_mask,
            cold.branch_masks.hash_mask,
            hashes,
            None,
        )
    }

    /// Returns the [`BranchNodeMasks`] for a branch based on the status of its children.
    pub(super) fn get_branch_masks(&self, idx: Index) -> BranchNodeMasks {
        let hot = self[idx].branch_ref();
        let cold = self.cold(idx).branch();
        let mut masks = BranchNodeMasks::default();

        for nibble in hot.state_mask.iter() {
            let (hash_bit, tree_bit) = if hot.revealed_mask.is_bit_set(nibble) {
                let child_idx = hot.children[nibble as usize];
                (self.hash_mask_bit(child_idx), self.tree_mask_bit(child_idx))
            } else {
                (
                    cold.branch_masks.hash_mask.is_bit_set(nibble),
                    cold.branch_masks.tree_mask.is_bit_set(nibble),
                )
            };
            masks.set_child_bits(nibble, hash_bit, tree_bit);
        }

        masks
    }

    /// Returns the blinded `RlpNode` at `nibble`, or `None` if the child is not blinded.
    pub(super) fn blinded_child(&self, idx: Index, nibble: u8) -> Option<&RlpNode> {
        let hot = self[idx].branch_ref();
        if !hot.is_child_blinded(nibble) {
            return None;
        }
        let dense = blinded_dense_index(hot, nibble);
        Some(&self.cold(idx).branch().blinded[dense])
    }

    /// Removes a child at `nibble`, handling both revealed and blinded cases.
    /// Marks the branch as dirty.
    pub(super) fn remove_branch_child(&mut self, idx: Index, nibble: u8) {
        let hot = self[idx].branch_mut();
        if hot.is_child_blinded(nibble) {
            let dense = blinded_dense_index(hot, nibble);
            self.cold_mut(idx).branch_mut().blinded.remove(dense);
        }
        self[idx].branch_mut().remove_child(nibble);
        *self.cold_mut(idx).state_mut() = ArenaSparseNodeState::Dirty;
    }

    /// Reveals a blinded child: removes the blinded RlpNode and sets the child as revealed.
    /// Does NOT mark the branch as dirty (revealing preserves cached state).
    pub(super) fn reveal_child(&mut self, idx: Index, nibble: u8, child_idx: Index) {
        let hot = self[idx].branch_mut();
        debug_assert!(hot.is_child_blinded(nibble), "reveal_child: nibble is not blinded");
        // Remove from blinded dense array first (while state_mask still has the bit set
        // and revealed_mask does not).
        let dense = blinded_dense_index(hot, nibble);
        self.cold_mut(idx).branch_mut().blinded.remove(dense);
        // Now set as revealed.
        self[idx].branch_mut().set_revealed_child(nibble, child_idx);
    }

    /// Blinds a revealed child: removes from the revealed set and stores its RlpNode.
    pub(super) fn blind_child(&mut self, idx: Index, nibble: u8, rlp: RlpNode) {
        let hot = self[idx].branch_mut();
        debug_assert!(
            hot.revealed_mask.is_bit_set(nibble),
            "blind_child: nibble is not revealed"
        );
        hot.revealed_mask.unset_bit(nibble);
        hot.children[nibble as usize] = null_index();
        // Insert into blinded dense array at the correct position.
        let dense = blinded_dense_index(&self[idx].branch_ref(), nibble);
        self.cold_mut(idx).branch_mut().blinded.insert(dense, rlp);
    }
}

/// Converts a [`ProofTrieNodeV2`] into hot and cold arena node parts.
///
/// # Panics
///
/// Panics if the node is an `Extension`, which should have been merged into a branch
/// by [`TrieNodeV2`].
pub(super) fn from_proof_node(
    proof_node: ProofTrieNodeV2,
) -> (ArenaSparseNode, Option<ArenaSparseNodeCold>) {
    let ProofTrieNodeV2 { node, masks, .. } = proof_node;
    match node {
        TrieNodeV2::EmptyRoot => (ArenaSparseNode::EmptyRoot, None),
        TrieNodeV2::Leaf(leaf) => (
            ArenaSparseNode::Leaf { key: leaf.key },
            Some(ArenaSparseNodeCold::Leaf {
                state: ArenaSparseNodeState::Revealed,
                value: leaf.value,
            }),
        ),
        TrieNodeV2::Branch(branch) => {
            let children = null_children();
            let blinded: SmallVec<[RlpNode; 4]> = branch.stack
                [..branch.state_mask.count_bits() as usize]
                .iter()
                .map(|rlp| rlp.clone())
                .collect();
            (
                ArenaSparseNode::Branch(ArenaBranchHot {
                    state_mask: branch.state_mask,
                    revealed_mask: TrieMask::default(),
                    short_key: branch.key,
                    children,
                }),
                Some(ArenaSparseNodeCold::Branch(ArenaBranchCold {
                    state: ArenaSparseNodeState::Revealed,
                    branch_masks: masks.unwrap_or_default(),
                    blinded,
                })),
            )
        }
        TrieNodeV2::Extension(_) => {
            panic!("Extension nodes should be merged into branches by TrieNodeV2")
        }
    }
}

/// Returns the dense index of `nibble` within the blinded children array.
///
/// The blinded mask is `state_mask & !revealed_mask`. The dense index is the number of
/// set bits in the blinded mask below `nibble`.
pub(super) fn blinded_dense_index(hot: &ArenaBranchHot, nibble: u8) -> usize {
    let blinded_mask = hot.state_mask.get() & !hot.revealed_mask.get();
    (blinded_mask & ((1u16 << nibble) - 1)).count_ones() as usize
}
