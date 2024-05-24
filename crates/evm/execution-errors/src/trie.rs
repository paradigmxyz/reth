//! Errors when computing the state root.

use reth_storage_errors::db::DatabaseError;
use thiserror::Error;

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
            StateRootError::DB(err) => err,
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
