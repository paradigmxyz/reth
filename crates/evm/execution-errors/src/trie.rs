//! Errors when computing the state root.

use alloc::string::ToString;
use alloy_primitives::B256;
use derive_more::Display;
use nybbles::Nibbles;
use reth_storage_errors::{db::DatabaseError, provider::ProviderError};

/// State root errors.
#[derive(Display, Debug, PartialEq, Eq, Clone)]
pub enum StateRootError {
    /// Internal database error.
    Database(DatabaseError),
    /// Storage root error.
    StorageRootError(StorageRootError),
}

impl From<DatabaseError> for StateRootError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<StorageRootError> for StateRootError {
    fn from(error: StorageRootError) -> Self {
        Self::StorageRootError(error)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StateRootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(source) => std::error::Error::source(source),
            Self::StorageRootError(source) => std::error::Error::source(source),
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
#[derive(Display, PartialEq, Eq, Clone, Debug)]
pub enum StorageRootError {
    /// Internal database error.
    Database(DatabaseError),
}

impl From<DatabaseError> for StorageRootError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<StorageRootError> for DatabaseError {
    fn from(err: StorageRootError) -> Self {
        match err {
            StorageRootError::Database(err) => err,
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StorageRootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(source) => std::error::Error::source(source),
        }
    }
}

/// State proof errors.
#[derive(Display, Debug, PartialEq, Eq, Clone)]
pub enum StateProofError {
    /// Internal database error.
    Database(DatabaseError),
    /// RLP decoding error.
    Rlp(alloy_rlp::Error),
}

impl From<DatabaseError> for StateProofError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<alloy_rlp::Error> for StateProofError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Rlp(error)
    }
}

impl From<StateProofError> for ProviderError {
    fn from(value: StateProofError) -> Self {
        match value {
            StateProofError::Database(error) => Self::Database(error),
            StateProofError::Rlp(error) => Self::Rlp(error),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StateProofError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(source) => std::error::Error::source(source),
            Self::Rlp(source) => std::error::Error::source(source),
        }
    }
}

/// Trie witness errors.
#[derive(Display, Debug, PartialEq, Eq, Clone)]
pub enum TrieWitnessError {
    /// Error gather proofs.
    Proof(StateProofError),
    /// RLP decoding error.
    Rlp(alloy_rlp::Error),
    /// Missing account.
    #[display("missing account {_0}")]
    MissingAccount(B256),
    /// Missing target node.
    #[display("target node missing from proof {_0:?}")]
    MissingTargetNode(Nibbles),
}

impl From<StateProofError> for TrieWitnessError {
    fn from(error: StateProofError) -> Self {
        Self::Proof(error)
    }
}

impl From<alloy_rlp::Error> for TrieWitnessError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Rlp(error)
    }
}

impl From<TrieWitnessError> for ProviderError {
    fn from(error: TrieWitnessError) -> Self {
        Self::TrieWitnessError(error.to_string())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TrieWitnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Proof(source) => std::error::Error::source(source),
            Self::Rlp(source) => std::error::Error::source(source),
            _ => Option::None,
        }
    }
}
