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
    /// The consumer dropped its cancel guard without waiting for the result.
    #[error("state root task canceled: consumer dropped the handle")]
    Canceled,
    /// Other unspecified error.
    #[error("{_0}")]
    Other(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl From<StateProofError> for StateRootTaskError {
    fn from(error: StateProofError) -> Self {
        Self::Provider(error.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_inconsistency_has_the_same_provider_classification() {
        let error = StateProofError::TrieInconsistency("cached node disagrees with leaf".into());
        let ordinary = ProviderError::from(error.clone());
        let StateRootTaskError::Provider(parallel) = StateRootTaskError::from(error) else {
            panic!("provider error expected")
        };
        assert_eq!(ordinary.to_string(), parallel.to_string());
        assert_eq!(ordinary.to_string(), "trie inconsistency: cached node disagrees with leaf");
        assert!(matches!(ordinary, ProviderError::Other(_)));
        assert!(matches!(parallel, ProviderError::Other(_)));
    }
}
