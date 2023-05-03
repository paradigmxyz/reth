use thiserror::Error;

/// State root error.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum StateRootError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] reth_db::Error),
    /// Storage root error.
    #[error(transparent)]
    StorageRootError(#[from] StorageRootError),
}

impl From<StateRootError> for reth_db::Error {
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
    DB(#[from] reth_db::Error),
}
