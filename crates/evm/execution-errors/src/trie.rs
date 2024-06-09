//! Errors when computing the state root.
use core::fmt::{Display, Formatter, Result};
use reth_storage_errors::db::DatabaseError;

/// State root errors.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum StateRootError {
    /// Internal database error.
    DB(DatabaseError),
    /// Storage root error.
    StorageRootError(StorageRootError),
}

#[cfg(feature = "std")]
impl std::error::Error for StateRootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DB(db_err) => Some(db_err),
            Self::StorageRootError(storage_err) => Some(storage_err),
        }
    }
}

impl Display for StateRootError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::DB(db_err) => Display::fmt(db_err, f),
            Self::StorageRootError(storage_err) => Display::fmt(storage_err, f),
        }
    }
}

impl From<DatabaseError> for StateRootError {
    fn from(err: DatabaseError) -> Self {
        Self::DB(err)
    }
}

impl From<StorageRootError> for StateRootError {
    fn from(err: StorageRootError) -> Self {
        Self::StorageRootError(err)
    }
}

// Implementing From<StateRootError> for DatabaseError
impl From<StateRootError> for DatabaseError {
    fn from(err: StateRootError) -> Self {
        match err {
            StateRootError::DB(db_err) => db_err,
            StateRootError::StorageRootError(storage_err) => match storage_err {
                StorageRootError::DB(db_err) => db_err,
            },
        }
    }
}

/// Storage root error.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum StorageRootError {
    /// Internal database error
    DB(DatabaseError),
}

#[cfg(feature = "std")]
impl std::error::Error for StorageRootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DB(db_err) => Some(db_err),
        }
    }
}

#[allow(unused_qualifications)]
impl Display for StorageRootError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::DB(db_err) => Display::fmt(db_err, f),
        }
    }
}

impl From<DatabaseError> for StorageRootError {
    fn from(err: DatabaseError) -> Self {
        Self::DB(err)
    }
}
