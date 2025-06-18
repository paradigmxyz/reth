use alloy_primitives::B256;
use alloy_trie::{Nibbles, TrieMask};
use reth_trie_common::TrieNode;

/// Enum representing sparse trie node type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparseNodeType {
    /// Empty trie node.
    Empty,
    /// A placeholder that stores only the hash for a node that has not been fully revealed.
    Hash,
    /// Sparse leaf node.
    Leaf,
    /// Sparse extension node.
    Extension {
        /// A flag indicating whether the extension node should be stored in the database.
        store_in_db_trie: Option<bool>,
    },
    /// Sparse branch node.
    Branch {
        /// A flag indicating whether the branch node should be stored in the database.
        store_in_db_trie: Option<bool>,
    },
}

impl SparseNodeType {
    pub const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash)
    }

    pub const fn is_branch(&self) -> bool {
        matches!(self, Self::Branch { .. })
    }

    pub const fn store_in_db_trie(&self) -> Option<bool> {
        match *self {
            Self::Extension { store_in_db_trie } | Self::Branch { store_in_db_trie } => {
                store_in_db_trie
            }
            _ => None,
        }
    }
}

/// Enum representing trie nodes in sparse trie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparseNode {
    /// Empty trie node.
    Empty,
    /// The hash of the node that was not revealed.
    Hash(B256),
    /// Sparse leaf node with remaining key suffix.
    Leaf {
        /// Remaining key suffix for the leaf node.
        key: Nibbles,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        hash: Option<B256>,
    },
    /// Sparse extension node with key.
    Extension {
        /// The key slice stored by this extension node.
        key: Nibbles,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        store_in_db_trie: Option<bool>,
    },
    /// Sparse branch node with state mask.
    Branch {
        /// The bitmask representing children present in the branch node.
        state_mask: TrieMask,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        store_in_db_trie: Option<bool>,
    },
}

impl SparseNode {
    /// Create new sparse node from [`TrieNode`].
    pub fn from_node(node: TrieNode) -> Self {
        match node {
            TrieNode::EmptyRoot => Self::Empty,
            TrieNode::Leaf(leaf) => Self::new_leaf(leaf.key),
            TrieNode::Extension(ext) => Self::new_ext(ext.key),
            TrieNode::Branch(branch) => Self::new_branch(branch.state_mask),
        }
    }

    /// Create new [`SparseNode::Branch`] from state mask.
    pub const fn new_branch(state_mask: TrieMask) -> Self {
        Self::Branch { state_mask, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Branch`] with two bits set.
    pub const fn new_split_branch(bit_a: u8, bit_b: u8) -> Self {
        let state_mask = TrieMask::new(
            // set bits for both children
            (1u16 << bit_a) | (1u16 << bit_b),
        );
        Self::Branch { state_mask, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Extension`] from the key slice.
    pub const fn new_ext(key: Nibbles) -> Self {
        Self::Extension { key, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Leaf`] from leaf key and value.
    pub const fn new_leaf(key: Nibbles) -> Self {
        Self::Leaf { key, hash: None }
    }

    /// Returns `true` if the node is a hash node.
    pub const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash(_))
    }
}

/// A helper struct used to store information about a node that has been removed
/// during a deletion operation.
#[derive(Debug)]
pub struct RemovedSparseNode {
    /// The path at which the node was located.
    pub path: Nibbles,
    /// The removed node
    pub node: SparseNode,
    /// For branch nodes, an optional nibble that should be unset due to the node being removed.
    ///
    /// During leaf deletion, this identifies the specific branch nibble path that
    /// connects to the leaf being deleted. Then when restructuring the trie after deletion,
    /// this nibble position will be cleared from the branch node's to
    /// indicate that the child no longer exists.
    ///
    /// This is only set for branch nodes that have a direct path to the leaf being deleted.
    pub unset_branch_nibble: Option<u8>,
}
