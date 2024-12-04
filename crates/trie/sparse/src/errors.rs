//! Errors for sparse trie.

use crate::SparseNode;
use alloy_primitives::{Bytes, B256};
use reth_trie_common::Nibbles;
use thiserror::Error;

/// Result type with [`SparseStateTrieError`] as error.
pub type SparseStateTrieResult<Ok> = Result<Ok, SparseStateTrieError>;

/// Error encountered in [`crate::SparseStateTrie`].
#[derive(Error, Debug)]
pub enum SparseStateTrieError {
    /// Encountered invalid root node.
    #[error("invalid root node at {path:?}: {node:?}")]
    InvalidRootNode {
        /// Path to first proof node.
        path: Nibbles,
        /// Encoded first proof node.
        node: Bytes,
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
    /// Other.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error>),
}
