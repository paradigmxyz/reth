//! Types related to sparse trie nodes and masks.

use crate::Nibbles;
use alloy_primitives::map::HashMap;
use alloy_trie::{nodes::TrieNode, TrieMask};

/// Branch node masks containing `hash_mask` and `tree_mask`.
///
/// Consolidates `hash_mask` and `tree_mask` into a single struct, reducing `HashMap` overhead
/// when storing masks by path. Instead of two separate `HashMap<Nibbles, TrieMask>`,
/// we use a single `HashMap<Nibbles, BranchNodeMasks>`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BranchNodeMasks {
    /// Hash mask indicating which children are stored as hashes.
    pub hash_mask: TrieMask,
    /// Tree mask indicating which children are complete subtrees.
    pub tree_mask: TrieMask,
}

/// Map from nibble path to branch node masks.
pub type BranchNodeMasksMap = HashMap<Nibbles, BranchNodeMasks>;

/// Struct for passing around branch node mask information.
///
/// Branch nodes can have up to 16 children (one for each nibble).
/// The masks represent which children are stored in different ways:
/// - `hash_mask`: Indicates which children are stored as hashes in the database
/// - `tree_mask`: Indicates which children are complete subtrees stored in the database
///
/// These masks are essential for efficient trie traversal and serialization, as they
/// determine how nodes should be encoded and stored on disk.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct TrieMasks {
    /// Branch node hash mask, if any.
    ///
    /// When a bit is set, the corresponding child node's hash is stored in the trie.
    ///
    /// This mask enables selective hashing of child nodes.
    pub hash_mask: Option<TrieMask>,
    /// Branch node tree mask, if any.
    ///
    /// When a bit is set, the corresponding child subtree is stored in the database.
    pub tree_mask: Option<TrieMask>,
}

impl TrieMasks {
    /// Helper function, returns both fields `hash_mask` and `tree_mask` as [`None`]
    pub const fn none() -> Self {
        Self { hash_mask: None, tree_mask: None }
    }
}

/// Carries all information needed by a sparse trie to reveal a particular node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofTrieNode {
    /// Path of the node.
    pub path: Nibbles,
    /// The node itself.
    pub node: TrieNode,
    /// Tree and hash masks for the node, if known.
    /// Both masks are always set together (from database branch nodes).
    pub masks: Option<BranchNodeMasks>,
}
