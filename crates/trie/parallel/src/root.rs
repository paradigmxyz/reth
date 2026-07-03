use reth_execution_errors::{SparseTrieError, StateProofError, StorageRootError};
use reth_provider::ProviderError;
use reth_storage_errors::db::DatabaseError;
use thiserror::Error;

/// Error during parallel state root calculation.
#[derive(Error, Debug)]
pub enum ParallelStateRootError {
    /// Error while calculating storage root.
    #[error(transparent)]
    StorageRoot(#[from] StorageRootError),
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Sparse trie error.
    #[error(transparent)]
    SparseTrie(#[from] SparseTrieError),
    /// Other unspecified error.
    #[error("{_0}")]
    Other(String),
}

impl From<ParallelStateRootError> for ProviderError {
    fn from(error: ParallelStateRootError) -> Self {
        match error {
            ParallelStateRootError::Provider(error) => error,
            ParallelStateRootError::StorageRoot(StorageRootError::Database(error)) => {
                Self::Database(error)
            }
            ParallelStateRootError::SparseTrie(error) => Self::other(error),
            ParallelStateRootError::Other(other) => Self::Database(DatabaseError::Other(other)),
        }
    }
}

impl From<alloy_rlp::Error> for ParallelStateRootError {
    fn from(error: alloy_rlp::Error) -> Self {
        Self::Provider(ProviderError::Rlp(error))
    }
}

impl From<StateProofError> for ParallelStateRootError {
    fn from(error: StateProofError) -> Self {
        match error {
            StateProofError::Database(err) => Self::Provider(ProviderError::Database(err)),
            StateProofError::Rlp(err) => Self::Provider(ProviderError::Rlp(err)),
            StateProofError::TrieInconsistency(msg) => {
                Self::Provider(ProviderError::TrieWitnessError(msg))
            }
        }
    }
}
