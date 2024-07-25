//! Errors when computing the state root.

use reth_storage_errors::{db::DatabaseError, provider::ProviderError};
use thiserror_no_std::Error;

/// State root errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum StateProofError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] DatabaseError),
    /// RLP decoding error.
    #[error(transparent)]
    RLP(#[from] alloy_rlp::Error),
}

impl From<StateProofError> for ProviderError {
    fn from(value: StateProofError) -> Self {
        match value {
            StateProofError::DB(error) => ProviderError::Database(error),
            StateProofError::RLP(error) => ProviderError::RLP(error),
        }
    }
}

/// State root errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum StateRootError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] DatabaseError),
    /// Storage root error.
    #[error(transparent)]
    StorageRootError(#[from] StorageRootError),
}

impl From<StateRootError> for DatabaseError {
    fn from(err: StateRootError) -> Self {
        match err {
            StateRootError::DB(err) |
            StateRootError::StorageRootError(StorageRootError::DB(err)) => err,
        }
    }
}

/// Storage root error.
#[derive(Error, PartialEq, Eq, Clone, Debug)]
pub enum StorageRootError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] DatabaseError),
}
