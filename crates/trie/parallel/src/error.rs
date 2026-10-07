use reth_execution_errors::{SparseTrieError, StateProofError};
use reth_provider::ProviderError;
use thiserror::Error;

/// Error returned by the state-root task and the parallel proof workers.
#[derive(Error, Debug)]
pub enum StateRootTaskError {
    /// Provider error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Proof dispatch error.
    #[error("proof dispatch failed: {_0}")]
    ProofDispatch(ProviderError),
    /// A proof worker failed before it could process queued work.
    #[error("proof worker failed: {_0}")]
    ProofWorker(String),
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
    Other(String),
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
    fn trie_inconsistency_keeps_its_type_across_proof_paths() {
        let proof_error =
            StateProofError::TrieInconsistency("cached node differs from leaf".into());
        let direct = ProviderError::from(proof_error.clone());
        let StateRootTaskError::Provider(parallel) = StateRootTaskError::from(proof_error) else {
            panic!("parallel proof error must use the shared provider conversion");
        };
        for error in [direct, parallel] {
            assert!(matches!(
                error.downcast_other_ref::<StateProofError>(),
                Some(StateProofError::TrieInconsistency(_))
            ));
            assert_eq!(error.to_string(), "trie inconsistency: cached node differs from leaf");
        }
    }
}
