//! Errors when computing the state root.

use reth_storage_errors::{db::DatabaseError, provider::ProviderError};
use thiserror_no_std::Error;

/// State root errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
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

/// State root errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
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
