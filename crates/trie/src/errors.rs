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

/// Storage root error.
#[derive(Error, Debug)]
pub enum StorageRootError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] reth_db::Error),
}
