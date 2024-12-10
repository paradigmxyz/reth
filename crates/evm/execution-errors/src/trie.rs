//! Errors when computing the state root.

use alloc::{boxed::Box, string::ToString};
use alloy_primitives::{Bytes, B256};
use nybbles::Nibbles;
use reth_storage_errors::{db::DatabaseError, provider::ProviderError};
use thiserror::Error;

/// State root errors.
#[derive(Error, PartialEq, Eq, Clone, Debug)]
pub enum StateRootError {
    /// Internal database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Storage root error.
    #[error(transparent)]
    StorageRootError(#[from] StorageRootError),
}

impl From<StateRootError> for DatabaseError {
    fn from(err: StateRootError) -> Self {
        match err {
            StateRootError::Database(err) |
            StateRootError::StorageRootError(StorageRootError::Database(err)) => err,
        }
    }
}

/// Storage root error.
#[derive(Error, PartialEq, Eq, Clone, Debug)]
pub enum StorageRootError {
    /// Internal database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

impl From<StorageRootError> for DatabaseError {
    fn from(err: StorageRootError) -> Self {
        match err {
            StorageRootError::Database(err) => err,
        }
    }
}

/// State proof errors.
#[derive(Error, PartialEq, Eq, Clone, Debug)]
pub enum StateProofError {
    /// Internal database error.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// RLP decoding error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
}

impl From<StateProofError> for ProviderError {
    fn from(value: StateProofError) -> Self {
        match value {
            StateProofError::Database(error) => Self::Database(error),
            StateProofError::Rlp(error) => Self::Rlp(error),
        }
    }
}

/// Result type with [`SparseStateTrieError`] as error.
pub type SparseStateTrieResult<Ok> = Result<Ok, SparseStateTrieError>;

/// Error encountered in `SparseStateTrie`.
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

/// Error encountered in `SparseTrie`.
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
        node: Box<dyn core::fmt::Debug + Send>,
    },
    /// RLP error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
    /// Other.
    #[error(transparent)]
    Other(#[from] Box<dyn core::error::Error + Send>),
}

/// Trie witness errors.
#[derive(Error, Debug)]
pub enum TrieWitnessError {
    /// Error gather proofs.
    #[error(transparent)]
    Proof(#[from] StateProofError),
    /// RLP decoding error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
    /// Sparse state trie error.
    #[error(transparent)]
    Sparse(#[from] SparseStateTrieError),
    /// Missing account.
    #[error("missing account {_0}")]
    MissingAccount(B256),
}

impl From<TrieWitnessError> for ProviderError {
    fn from(error: TrieWitnessError) -> Self {
        Self::TrieWitnessError(error.to_string())
    }
}
