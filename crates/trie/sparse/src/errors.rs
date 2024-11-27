//! Errors for sparse trie.

use alloy_primitives::B256;
use alloy_trie::nodes::TrieNode;
use reth_trie_common::Nibbles;
use thiserror::Error;

use crate::SparseNode;

/// Result type with [`SparseStateTrieError`] as error.
pub type SparseStateTrieResult<Ok> = Result<Ok, SparseStateTrieError>;

/// Error encountered in [`crate::SparseStateTrie`].
#[derive(Error, Debug)]
pub enum SparseStateTrieError {
    /// Encountered invalid root node path.
    #[error("invalid root node at {path:?}: {node:?}")]
    InvalidRootNodePath {
        /// Path to first proof node.
        path: Nibbles,
        /// Decoded first proof node.
        node: Box<TrieNode>,
    },
    /// Encountered unexpected node at path.
    #[error("encountered unexpected node at path {path:?} when revealing: {node:?}")]
    UnexpectedNode {
        /// Path to the node.
        path: Nibbles,
        /// Node that was at the path when revealing.
        node: Box<TrieNode>,
    },
    /// Encountered proof without leaf node.
    #[error("proof without leaf node at {path:?}: {node:?}")]
    ProofWithoutLeafNode {
        /// Path to the leaf node.
        path: Nibbles,
        /// Decoded leaf node.
        node: Box<TrieNode>,
    },
    /// Sparse trie error.
    #[error(transparent)]
    Sparse(#[from] SparseTrieError),
    /// RLP error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}

/// Result type with [`SparseTrieError`] as error.
pub type SparseTrieResult<Ok> = Result<Ok, SparseTrieError>;

/// Error encountered in [`crate::SparseTrie`].
#[derive(Error, Debug)]
pub enum SparseTrieError {
    /// Sparse trie is still blind. Thrown on attempt to update it.
    #[error("sparse trie is blind")]
    Blind,
    /// Encountered blinded node on update.
    #[error("attempted to update blind node at {path:?}: {hash}")]
    BlindedNode {
        /// Blind node path.
        path: Nibbles,
        /// Node hash
        hash: B256,
    },
    /// Encountered unexpected node at path when revealing.
    #[error("encountered an invalid node at path {path:?} when revealing: {node:?}")]
    Reveal {
        /// Path to the node.
        path: Nibbles,
        /// Node that was at the path when revealing.
        node: Box<SparseNode>,
    },
    /// RLP error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}
