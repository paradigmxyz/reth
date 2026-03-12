use super::{branch_child_idx::BranchChildIdx, ArenaSparseSubtrie};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::{keccak256, B256};
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};
use slotmap::{DefaultKey, Key as _, SlotMap};

/// Alias for the slotmap key type used as node references throughout the arena trie.
type Index = DefaultKey;
/// Alias for the slotmap used as the node arena throughout the arena trie.
type NodeArena = SlotMap<Index, ArenaSparseNode>;

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

/// The branch-specific data stored in an [`ArenaSparseNode::Branch`].
///
/// Field order is chosen so the seek hot path (masks, short_key, children) occupies
/// the first ~180 bytes, keeping cold data (state, blinded_children) at the end.
/// `#[repr(C)]` prevents the compiler from reordering fields.
#[derive(Debug, Clone)]
#[repr(C)]
pub(super) struct ArenaSparseNodeBranch {
    /// Bitmask indicating which of the 16 child slots are occupied.
    pub(super) state_mask: TrieMask,
    /// Which occupied children are revealed (present in the arena).
    pub(super) revealed_mask: TrieMask,
    /// Tree mask and hash mask for database persistence (`TrieUpdates`).
    pub(super) branch_masks: BranchNodeMasks,
    /// The short key (extension key) for this branch.
    pub(super) short_key: Nibbles,
    /// Direct nibble-to-Index lookup. Valid where `revealed_mask` bit is set.
    pub(super) children: [Index; 16],
    /// Cached or dirty state of this node.
    pub(super) state: ArenaSparseNodeState,
    /// Blinded children's RLP nodes, packed densely by `BranchChildIdx` on the blinded mask.
    /// Heap-allocated to keep the branch struct compact for cache performance.
    pub(super) blinded_children: Vec<RlpNode>,
}

impl ArenaSparseNodeBranch {
    /// Unsets the bit for `nibble` in `state_mask`, `hash_mask`, and `tree_mask`.
    pub(super) const fn unset_child_bit(&mut self, nibble: u8) {
        self.state_mask.unset_bit(nibble);
    }

    /// Inserts a child at `nibble`, updating the state mask, children array, and marking the
    /// branch as dirty.
    pub(super) fn set_child(&mut self, nibble: u8, child: ArenaSparseNodeBranchChild) {
        match child {
            ArenaSparseNodeBranchChild::Revealed(idx) => {
                if !self.state_mask.is_bit_set(nibble) {
                    self.state_mask.set_bit(nibble);
                }
                self.revealed_mask.set_bit(nibble);
                self.children[nibble as usize] = idx;
            }
            ArenaSparseNodeBranchChild::Blinded(rlp) => {
                let was_set = self.state_mask.is_bit_set(nibble);
                if !was_set {
                    self.state_mask.set_bit(nibble);
                }
                // Remove from revealed if it was revealed
                if self.revealed_mask.is_bit_set(nibble) {
                    self.revealed_mask.unset_bit(nibble);
                }
                // Insert into blinded_children at the correct dense position
                let blinded_mask = self.blinded_mask();
                let insert_pos = BranchChildIdx::new(blinded_mask, nibble)
                    .expect("nibble must be in blinded mask");
                self.blinded_children.insert(insert_pos.get(), rlp);
            }
        }
        self.state = ArenaSparseNodeState::Dirty;
    }

    /// Removes the child at `nibble`, updating the state mask, children array, and marking the
    /// branch as dirty.
    ///
    /// # Panics
    ///
    /// Panics if `nibble` is not set in the state mask.
    pub(super) fn remove_child(&mut self, nibble: u8) {
        debug_assert!(self.state_mask.is_bit_set(nibble), "nibble not found in state_mask");
        if self.revealed_mask.is_bit_set(nibble) {
            self.revealed_mask.unset_bit(nibble);
        } else {
            // Remove from blinded_children
            let blinded_mask = self.blinded_mask();
            let child_idx = BranchChildIdx::new(blinded_mask, nibble)
                .expect("nibble not found in blinded mask");
            self.blinded_children.remove(child_idx.get());
        }
        self.unset_child_bit(nibble);
        self.state = ArenaSparseNodeState::Dirty;
    }

    /// Iterates over `(nibble, ArenaSparseNodeBranchChild)` pairs in nibble order.
    pub(super) fn child_iter(&self) -> impl Iterator<Item = (u8, ArenaSparseNodeBranchChild)> + '_ {
        let mut blinded_idx = 0usize;
        self.state_mask.iter().map(move |nibble| {
            if self.revealed_mask.is_bit_set(nibble) {
                (nibble, ArenaSparseNodeBranchChild::Revealed(self.children[nibble as usize]))
            } else {
                let rlp = self.blinded_children[blinded_idx].clone();
                blinded_idx += 1;
                (nibble, ArenaSparseNodeBranchChild::Blinded(rlp))
            }
        })
    }

    /// Returns a [`BranchNodeCompact`] from this branch's masks and children hashes.
    pub(super) fn branch_node_compact(&self, arena: &NodeArena) -> BranchNodeCompact {
        let mut hashes = Vec::new();
        for (nibble, child) in self.child_iter() {
            if self.branch_masks.hash_mask.is_bit_set(nibble) {
                let hash = match &child {
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

    /// Returns the child at `nibble` as an `ArenaSparseNodeBranchChild`.
    /// Panics if nibble is not set in `state_mask`.
    pub(super) fn get_child(&self, nibble: u8) -> ArenaSparseNodeBranchChild {
        debug_assert!(self.state_mask.is_bit_set(nibble));
        if self.revealed_mask.is_bit_set(nibble) {
            ArenaSparseNodeBranchChild::Revealed(self.children[nibble as usize])
        } else {
            let blinded_mask = self.blinded_mask();
            let idx = BranchChildIdx::new(blinded_mask, nibble).expect("must be in blinded mask");
            ArenaSparseNodeBranchChild::Blinded(self.blinded_children[idx.get()].clone())
        }
    }

    /// Returns the blinded mask (children that are in `state_mask` but not revealed).
    pub(super) const fn blinded_mask(&self) -> TrieMask {
        TrieMask::new(self.state_mask.get() & !self.revealed_mask.get())
    }

    /// Returns the revealed child Index at `nibble`, or None if not revealed.
    pub(super) fn revealed_child(&self, nibble: u8) -> Option<Index> {
        self.revealed_mask.is_bit_set(nibble).then(|| self.children[nibble as usize])
    }

    /// Returns true if the child at `nibble` is blinded.
    pub(super) const fn is_child_blinded(&self, nibble: u8) -> bool {
        self.state_mask.is_bit_set(nibble) && !self.revealed_mask.is_bit_set(nibble)
    }

    /// Replaces a blinded child with a revealed one.
    pub(super) fn reveal_child(&mut self, nibble: u8, idx: Index) {
        debug_assert!(self.state_mask.is_bit_set(nibble), "nibble not in state_mask");
        debug_assert!(!self.revealed_mask.is_bit_set(nibble), "nibble already revealed");
        // Remove from blinded_children
        let blinded_mask = self.blinded_mask();
        let blinded_idx =
            BranchChildIdx::new(blinded_mask, nibble).expect("nibble must be in blinded mask");
        self.blinded_children.remove(blinded_idx.get());
        // Add to revealed
        self.revealed_mask.set_bit(nibble);
        self.children[nibble as usize] = idx;
    }

    /// Replaces a revealed child with a blinded one (for pruning).
    pub(super) fn blind_child(&mut self, nibble: u8, rlp_node: RlpNode) {
        debug_assert!(self.revealed_mask.is_bit_set(nibble), "nibble not revealed");
        self.revealed_mask.unset_bit(nibble);
        // Insert into blinded_children at the correct dense position
        let blinded_mask = self.blinded_mask();
        let insert_pos = BranchChildIdx::new(blinded_mask, nibble)
            .expect("nibble must be in blinded mask after unset");
        self.blinded_children.insert(insert_pos.get(), rlp_node);
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
                let blinded_children =
                    branch.stack[..branch.state_mask.count_bits() as usize].to_vec();
                Self::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed,
                    state_mask: branch.state_mask,
                    revealed_mask: TrieMask::default(),
                    short_key: branch.key,
                    children: [Index::null(); 16],
                    branch_masks: masks.unwrap_or_default(),
                    blinded_children,
                })
            }
            TrieNodeV2::Extension(_) => {
                panic!("Extension nodes should be merged into branches by TrieNodeV2")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_node_size() {
        assert!(
            core::mem::size_of::<ArenaSparseNode>() <= 248,
            "ArenaSparseNode grew to {} bytes (target: <= 248)",
            core::mem::size_of::<ArenaSparseNode>()
        );
    }
}
