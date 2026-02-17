use crate::{SparseTrieUpdates, SparseTrieUpdatesAction};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::B256;
use alloy_trie::TrieMask;
use reth_trie_common::{BranchNodeMasks, Nibbles, RlpNode};
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
    /// The root node of the upper trie, or `None` if the trie is empty/uninitialized.
    root: Option<Index>,
    /// Optional tracking of trie updates for database persistence.
    updates: Option<SparseTrieUpdates>,
    /// Reusable stack buffer for trie traversals.
    stack: Vec<Index>,
    /// Reusable buffer for collecting update actions.
    update_actions: Vec<SparseTrieUpdatesAction>,
    /// Pool of cleared `ArenaSparseSubtrie`s available for reuse.
    cleared_subtries: Vec<ArenaSparseSubtrie>,
}
