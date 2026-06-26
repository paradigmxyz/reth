use super::{
    branch_child_idx::{BranchChildIdx, BranchChildIter},
    ArenaSparseSubtrie, Index, NodeArena,
};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::{keccak256, B256};
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};
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
    /// Converts into a [`Self::Dirty`] if it's not already.
    pub(super) const fn to_dirty(&self) -> Self {
        Self::Dirty
    }

    /// Returns the [`RlpNode`] cached on the state, if there is one.
    pub(super) const fn cached_rlp_node(&self) -> Option<&RlpNode> {
        match self {
            Self::Cached { rlp_node, .. } => Some(rlp_node),
            _ => None,
        }
    }
}

/// Represents a reference from a branch node to one of its children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArenaSparseNodeBranchChild {
    /// The child node has been revealed and is present in the arena.
    Revealed(Index),
    /// The child node has not been revealed; only its RLP-encoded node is known.
    Blinded(RlpNode),
}

impl ArenaSparseNodeBranchChild {
    /// Returns `true` if this child reference is blinded (not yet revealed in the arena).
    pub(super) const fn is_blinded(&self) -> bool {
        matches!(self, Self::Blinded(_))
    }
}

/// The branch-specific data stored in an [`ArenaSparseNode::Branch`].
#[derive(Debug, Clone)]
pub(super) struct ArenaSparseNodeBranch {
    /// Cached or dirty state of this node.
    pub(super) state: ArenaSparseNodeState,
    /// Revealed or blinded children, packed densely. The `state_mask` tracks which
    /// nibble positions have entries in this `SmallVec`.
    pub(super) children: SmallVec<[ArenaSparseNodeBranchChild; 4]>,
    /// Bitmask indicating which of the 16 child slots are occupied (have an entry
    /// in `children`).
    pub(super) state_mask: TrieMask,
    /// The short key (extension key) for this branch. When non-empty, the node's path is the
    /// path of the parent extension node with this short key.
    pub(super) short_key: Nibbles,
    /// Tree mask and hash mask for database persistence (`TrieUpdates`).
    pub(super) branch_masks: BranchNodeMasks,
}

impl ArenaSparseNodeBranch {
    /// Unsets the bit for `nibble` in `state_mask`, `hash_mask`, and `tree_mask`.
    pub(super) const fn unset_child_bit(&mut self, nibble: u8) {
        self.state_mask.unset_bit(nibble);
    }

    /// Inserts a child at `nibble`, updating the state mask, children array, and marking the
    /// branch as dirty.
    pub(super) fn set_child(&mut self, nibble: u8, child: ArenaSparseNodeBranchChild) {
        let insert_pos = BranchChildIdx::insertion_point(self.state_mask, nibble);
        self.state_mask.set_bit(nibble);
        self.children.insert(insert_pos.get(), child);
        self.state = ArenaSparseNodeState::Dirty;
    }

    /// Removes the child at `nibble`, updating the state mask, children array, and marking the
    /// branch as dirty.
    ///
    /// # Panics
    ///
    /// Panics if `nibble` is not set in the state mask.
    pub(super) fn remove_child(&mut self, nibble: u8) {
        let child_idx =
            BranchChildIdx::new(self.state_mask, nibble).expect("nibble not found in state_mask");
        self.children.remove(child_idx.get());
        self.unset_child_bit(nibble);
        self.state = ArenaSparseNodeState::Dirty;
    }

    /// Returns a reference to the sibling child in a branch with exactly 2 children.
    ///
    /// # Panics
    ///
    /// Panics (debug) if the branch does not have exactly 2 children, or if `nibble` is not set.
    pub(super) fn sibling_child(&self, nibble: u8) -> &ArenaSparseNodeBranchChild {
        debug_assert_eq!(
            self.state_mask.count_bits(),
            2,
            "sibling_child requires exactly 2 children"
        );
        let child_idx =
            BranchChildIdx::new(self.state_mask, nibble).expect("nibble not found in state_mask");
        // With exactly 2 children the dense array has indices 0 and 1.
        &self.children[1 - child_idx.get()]
    }

    /// Iterates over `(nibble, &ArenaSparseNodeBranchChild)` pairs in nibble order.
    pub(super) fn child_iter(
        &self,
    ) -> impl Iterator<Item = (u8, &ArenaSparseNodeBranchChild)> + '_ {
        BranchChildIter::new(self.state_mask).map(|(idx, nibble)| (nibble, &self.children[idx]))
    }

    /// Returns a [`BranchNodeCompact`] from this branch's masks and children hashes.
    pub(super) fn branch_node_compact(&self, arena: &NodeArena) -> BranchNodeCompact {
        let mut hashes = Vec::new();
        for (nibble, child) in self.child_iter() {
            if self.branch_masks.hash_mask.is_bit_set(nibble) {
                let hash = match child {
                    ArenaSparseNodeBranchChild::Blinded(rlp_node) => {
                        rlp_node.as_hash().expect("blinded child must be a hash")
                    }
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                        arena[*child_idx].cached_hash()
                    }
                };
                hashes.push(hash);
            }
        }
        BranchNodeCompact::new(
            self.state_mask,
            self.branch_masks.tree_mask,
            self.branch_masks.hash_mask,
            hashes,
            None,
        )
    }
}

/// A node in the arena-based sparse trie.
#[derive(Debug, Clone)]
pub(super) enum ArenaSparseNode {
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
    /// Returns the state of a Branch, Leaf, or Subtrie root node, or `None` for other types.
    pub(super) fn state_ref(&self) -> Option<&ArenaSparseNodeState> {
        match self {
            Self::Branch(b) => Some(&b.state),
            Self::Leaf { state, .. } => Some(state),
            Self::Subtrie(s) => s.arena[s.root].state_ref(),
            _ => None,
        }
    }

    /// Returns a mutable reference to the state of a Branch or Leaf node.
    ///
    /// # Panics
    ///
    /// Panics if called on a non-Branch/Leaf node.
    pub(super) fn state_mut(&mut self) -> &mut ArenaSparseNodeState {
        match self {
            Self::Branch(b) => &mut b.state,
            Self::Leaf { state, .. } => state,
            _ => panic!("state_mut called on non-Branch/Leaf node"),
        }
    }

    /// Returns `true` if this node's RLP encoding is cached.
    pub(super) fn is_cached(&self) -> bool {
        self.state_ref().is_some_and(|s| matches!(s, ArenaSparseNodeState::Cached { .. }))
    }

    /// Returns the short key of the branch or leaf, or None.
    pub(super) const fn short_key(&self) -> Option<&Nibbles> {
        match self {
            Self::Branch(b) => Some(&b.short_key),
            Self::Leaf { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Returns a reference to the branch data.
    ///
    /// # Panics
    ///
    /// Panics if this is not a `Branch` node.
    pub(super) fn branch_ref(&self) -> &ArenaSparseNodeBranch {
        match self {
            Self::Branch(b) => b,
            _ => panic!("branch_ref called on non-Branch node {self:?}"),
        }
    }

    /// Returns a mutable reference to the branch data.
    ///
    /// # Panics
    ///
    /// Panics if this is not a `Branch` node.
    pub(super) fn branch_mut(&mut self) -> &mut ArenaSparseNodeBranch {
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

    /// Returns the branch data if this node (or its subtrie root) is a branch, or `None`.
    pub(super) fn as_branch(&self) -> Option<&ArenaSparseNodeBranch> {
        match self {
            Self::Branch(b) => Some(b),
            Self::Subtrie(s) => s.arena[s.root].as_branch(),
            _ => None,
        }
    }

    /// Returns `true` if this node should contribute a set bit in its parent's `hash_mask`.
    ///
    /// That is, if the node is a branch with no short key (no extension) whose cached
    /// RLP is a hash (>= 32 bytes). Small branches whose RLP is embedded don't get a
    /// `hash_mask` bit.
    pub(super) fn hash_mask_bit(&self) -> bool {
        self.as_branch().is_some_and(|b| {
            b.short_key.is_empty() &&
                b.state.cached_rlp_node().expect("branch's RlpNode must be cached").is_hash()
        })
    }

    /// Returns `true` if this node should contribute a set bit in its parent's `tree_mask`.
    ///
    /// That is, if the node is a branch with any non-empty `branch_masks`.
    pub(super) fn tree_mask_bit(&self) -> bool {
        self.as_branch().is_some_and(|b| !b.branch_masks.is_empty())
    }

    /// Returns the cached hash of this node. Panics if the node's state is not `Cached`.
    ///
    /// If the `RlpNode` is already a hash (>= 32 bytes encoded), returns it directly.
    /// Otherwise keccak-hashes the RLP encoding to produce the hash. This handles the
    /// case where a branch's RLP is small enough to be embedded rather than hashed.
    pub(super) fn cached_hash(&self) -> B256 {
        let rlp_node = match self {
            Self::Branch(ArenaSparseNodeBranch { state, .. }) | Self::Leaf { state, .. } => state
                .cached_rlp_node()
                .expect("cached_hash called on non-Cached branch or leaf: {self:?}"),
            Self::Subtrie(s) => return s.arena[s.root].cached_hash(),
            _ => panic!("cached_hash called on {self:?}"),
        };
        rlp_node.as_hash().unwrap_or_else(|| keccak256(rlp_node.as_slice()))
    }
}

impl ArenaSparseNode {
    /// Converts a [`ProofTrieNodeV2`] into an [`ArenaSparseNode`].
    ///
    /// # Panics
    ///
    /// Panics if the node is an `Extension`, which should have been merged into a branch
    /// by [`TrieNodeV2`].
    pub(super) fn from_proof_node(proof_node: ProofTrieNodeV2) -> Self {
        let ProofTrieNodeV2 { node, masks, .. } = proof_node;
        match node {
            TrieNodeV2::EmptyRoot => Self::EmptyRoot,
            TrieNodeV2::Leaf(leaf) => Self::Leaf {
                state: ArenaSparseNodeState::Revealed,
                key: leaf.key,
                value: leaf.value,
            },
            TrieNodeV2::Branch(branch) => {
                let children = branch.stack[..branch.state_mask.count_bits() as usize]
                    .iter()
                    .map(|rlp| ArenaSparseNodeBranchChild::Blinded(rlp.clone()))
                    .collect();
                Self::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed,
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

    /// Returns the heap bytes owned by this node beyond its inline `SlotMap` slot.
    pub(super) fn extra_heap_bytes(&self) -> usize {
        match self {
            Self::Leaf { value, .. } => value.capacity(),
            Self::Branch(b) if b.children.spilled() => {
                b.children.capacity() * core::mem::size_of::<ArenaSparseNodeBranchChild>()
            }
            _ => 0,
        }
    }
}
