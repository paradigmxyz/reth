use crate::{
    LeafLookup, LeafLookupError, LeafUpdate, SparseTrie, SparseTrieUpdates, SparseTrieUpdatesAction,
};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_primitives::{map::B256Map, B256};
use alloy_trie::TrieMask;
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};
use smallvec::SmallVec;
use thunderdome::{Arena, Index};

/// Tracks whether a node's RLP encoding is cached or needs recomputation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArenaSparseNodeState {
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
    const fn num_leaves(&self) -> u64 {
        match self {
            Self::Cached { num_leaves, .. } | Self::Dirty { num_leaves, .. } => *num_leaves,
        }
    }
}

/// Represents a reference from a branch node to one of its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArenaSparseNodeBranchChild {
    /// The child node has been revealed and is present in the arena.
    Revealed(Index),
    /// The child node has not been revealed; only its hash is known.
    Blinded(B256),
}

/// A node in the arena-based sparse trie.
#[derive(Debug)]
enum ArenaSparseNode {
    /// Indicates a trie with no nodes.
    EmptyRoot,
    /// A branch node with up to 16 children.
    Branch {
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
        /// Cached RLP encodings of children. The `cached_child_rlp_mask` tracks which
        /// children have cached RLP nodes stored here.
        cached_child_rlp_nodes: SmallVec<[RlpNode; 4]>,
        /// Bitmask indicating which children have cached RLP nodes in
        /// `cached_child_rlp_nodes`.
        cached_child_rlp_mask: TrieMask,
        /// Tree mask and hash mask for database persistence (TrieUpdates).
        branch_masks: BranchNodeMasks,
    },
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
            Self::Branch { state, .. } | Self::Leaf { state, .. } => state.num_leaves(),
            Self::Subtrie(s) => s.arena[s.root].num_leaves(),
        }
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
    stack: Vec<Index>,
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
    stack: Vec<Index>,
    /// Reusable buffer for collecting update actions.
    update_actions: Vec<SparseTrieUpdatesAction>,
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
            cleared_subtries: Vec::new(),
        }
    }
}

impl SparseTrie for ArenaParallelSparseTrie {
    fn set_root(
        &mut self,
        _root: TrieNodeV2,
        _masks: Option<BranchNodeMasks>,
        _retain_updates: bool,
    ) -> SparseTrieResult<()> {
        todo!()
    }

    fn set_updates(&mut self, retain_updates: bool) {
        self.updates = retain_updates.then(Default::default);
    }

    fn reserve_nodes(&mut self, _additional: usize) {
        // thunderdome::Arena does not support reserve; no-op.
    }

    fn reveal_nodes(&mut self, _nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        todo!()
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
            ArenaSparseNode::Branch { state: ArenaSparseNodeState::Cached { .. }, .. } |
                ArenaSparseNode::Leaf { state: ArenaSparseNodeState::Cached { .. }, .. }
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
        _updates: &mut B256Map<LeafUpdate>,
        _proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()> {
        todo!()
    }
}
