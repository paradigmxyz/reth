use reth_execution_errors::{SparseTrieError, StateProofError};
use reth_provider::ProviderError;
use thiserror::Error;

/// Error returned by the state-root task and the parallel proof workers.
#[derive(Error, Debug)]
pub enum StateRootTaskError {
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Sparse trie error.
    #[error(transparent)]
    SparseTrie(#[from] SparseTrieError),
    /// Sparse trie task stalled.
    #[error("sparse trie task stalled")]
    Stalled,
    /// Other unspecified error.
    #[error("{_0}")]
    Other(String),
}

impl From<StateProofError> for StateRootTaskError {
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
