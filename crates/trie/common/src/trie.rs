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

impl BranchNodeMasks {
    /// Creates masks from optional hash and tree masks.
    ///
    /// Returns `None` if both masks are `None`, otherwise defaults missing masks to empty.
    pub fn from_optional(hash_mask: Option<TrieMask>, tree_mask: Option<TrieMask>) -> Option<Self> {
        match (hash_mask, tree_mask) {
            (Some(h), Some(t)) => Some(Self { hash_mask: h, tree_mask: t }),
            (Some(h), None) => Some(Self { hash_mask: h, tree_mask: TrieMask::default() }),
            (None, Some(t)) => Some(Self { hash_mask: TrieMask::default(), tree_mask: t }),
            (None, None) => None,
        }
    }
}

/// Map from nibble path to branch node masks.
pub type BranchNodeMasksMap = HashMap<Nibbles, BranchNodeMasks>;

/// Carries all information needed by a sparse trie to reveal a particular node.
///
/// This is the legacy version that wraps alloy's [`TrieNode`]. Prefer [`ProofTrieNode`] (from
/// `trie_node_v2` module) which merges extension nodes into branch nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyProofTrieNode {
    /// Path of the node.
    pub path: Nibbles,
    /// The node itself.
    pub node: TrieNode,
    /// Tree and hash masks for the node, if known.
    /// Both masks are always set together (from database branch nodes).
    pub masks: Option<BranchNodeMasks>,
}
