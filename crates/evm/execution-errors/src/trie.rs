//! Errors when computing the state root.

use alloc::string::ToString;
use alloy_primitives::B256;
use derive_more::{Display, From};
use nybbles::Nibbles;
use reth_storage_errors::{db::DatabaseError, provider::ProviderError};

/// State root errors.
#[derive(Display, Debug, From, PartialEq, Eq, Clone)]
pub enum StateRootError {
    /// Internal database error.
    Database(DatabaseError),
    /// Storage root error.
    StorageRootError(StorageRootError),
}

impl core::error::Error for StateRootError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Database(source) => core::error::Error::source(source),
            Self::StorageRootError(source) => core::error::Error::source(source),
        }
    }
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
#[derive(Display, From, PartialEq, Eq, Clone, Debug)]
pub enum StorageRootError {
    /// Internal database error.
    Database(DatabaseError),
}

impl From<StorageRootError> for DatabaseError {
    fn from(err: StorageRootError) -> Self {
        match err {
            StorageRootError::Database(err) => err,
        }
    }
}

impl core::error::Error for StorageRootError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Database(source) => core::error::Error::source(source),
        }
    }
}

/// State proof errors.
#[derive(Display, From, Debug, PartialEq, Eq, Clone)]
pub enum StateProofError {
    /// Internal database error.
    Database(DatabaseError),
    /// RLP decoding error.
    Rlp(alloy_rlp::Error),
}

impl From<StateProofError> for ProviderError {
    fn from(value: StateProofError) -> Self {
        match value {
            StateProofError::Database(error) => Self::Database(error),
            StateProofError::Rlp(error) => Self::Rlp(error),
        }
    }
}

impl core::error::Error for StateProofError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Database(source) => core::error::Error::source(source),
            Self::Rlp(source) => core::error::Error::source(source),
        }
    }
}

/// Trie witness errors.
#[derive(Display, From, Debug, PartialEq, Eq, Clone)]
pub enum TrieWitnessError {
    /// Error gather proofs.
    #[from]
    Proof(StateProofError),
    /// RLP decoding error.
    #[from]
    Rlp(alloy_rlp::Error),
    /// Missing account.
    #[display("missing account {_0}")]
    MissingAccount(B256),
    /// Missing target node.
    #[display("target node missing from proof {_0:?}")]
    MissingTargetNode(Nibbles),
    /// Unexpected empty root.
    #[display("unexpected empty root: {_0:?}")]
    UnexpectedEmptyRoot(Nibbles),
}

impl From<TrieWitnessError> for ProviderError {
    fn from(error: TrieWitnessError) -> Self {
        Self::TrieWitnessError(error.to_string())
    }
}

impl core::error::Error for TrieWitnessError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Proof(source) => core::error::Error::source(source),
            Self::Rlp(source) => core::error::Error::source(source),
            _ => Option::None,
        }
    }
}
