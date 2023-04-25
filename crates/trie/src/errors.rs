use thiserror::Error;

/// State root error.
#[derive(Error, Debug)]
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
#[derive(Error, Debug)]
pub enum StorageRootError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] reth_db::Error),
}
