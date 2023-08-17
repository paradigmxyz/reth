use reth_primitives::H256;
use thiserror::Error;

/// State root error.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum StateRootError {
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] reth_db::DatabaseError),
    /// Storage root error.
    #[error(transparent)]
    StorageRootError(#[from] StorageRootError),
}

impl From<StateRootError> for reth_db::DatabaseError {
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
    DB(#[from] reth_db::DatabaseError),
}

/// Proof error.
#[derive(Error, PartialEq, Eq, Clone, Debug)]
pub enum ProofError {
    /// Leaf account missing
    #[error(
        "Expected leaf account with key greater or equal to {0:?} is missing from the database"
    )]
    LeafAccountMissing(H256),
    /// Storage root error.
    #[error(transparent)]
    StorageRootError(#[from] StorageRootError),
    /// Internal database error.
    #[error(transparent)]
    DB(#[from] reth_db::DatabaseError),
}
