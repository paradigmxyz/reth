//! Errors when computing the state root.

use reth_storage_errors::db::DatabaseError;
use thiserror_no_std::Error;

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

#[cfg(feature = "std")]
impl std::error::Error for StateRootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DB(err) => Some(err),
            Self::StorageRootError(err) => Some(err),
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

#[cfg(feature = "std")]
impl std::error::Error for StorageRootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DB(err) => Some(err),
        }
    }
}
