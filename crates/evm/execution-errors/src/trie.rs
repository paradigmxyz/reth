//! Errors when computing the state root.

use alloc::string::ToString;
use alloy_primitives::B256;
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

/// Trie witness errors.
#[derive(Error, PartialEq, Eq, Clone, Debug)]
pub enum TrieWitnessError {
    /// Error gather proofs.
    #[error(transparent)]
    Proof(#[from] StateProofError),
    /// RLP decoding error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
    /// Missing account.
    #[error("missing account {_0}")]
    MissingAccount(B256),
    /// Missing target node.
    #[error("target node missing from proof {_0:?}")]
    MissingTargetNode(Nibbles),
    /// Unexpected empty root.
    #[error("unexpected empty root: {_0:?}")]
    UnexpectedEmptyRoot(Nibbles),
}

impl From<TrieWitnessError> for ProviderError {
    fn from(error: TrieWitnessError) -> Self {
        Self::TrieWitnessError(error.to_string())
    }
}
