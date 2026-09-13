//! Work that runs on a storage proof worker while owning a storage trie.

use alloy_primitives::B256;
use reth_trie::{ProofTrieNodeV2, ProofV2Target};

pub use reth_execution_errors::StateProofError;

/// A unit of work that owns a storage trie and runs on a storage proof worker, next to the
/// database cursors that trie's proofs are read from.
///
/// Handing the trie to the worker collapses a proof round for it into a single hop: the worker
/// proves the targets the trie asks for and reveals them itself, instead of returning them to the
/// caller and waiting for a proof to travel back.
pub trait StorageTrieWorkerJob: Send {
    /// Runs the job.
    ///
    /// `prover` is `None` when no worker could take the job, in which case the job has to make do
    /// with what it can do without proofs.
    fn run(self: Box<Self>, prover: Option<&mut dyn StorageProver>);
}

/// Computes storage proofs on the database cursors of a storage proof worker.
pub trait StorageProver {
    /// Returns the proof nodes for `targets` in the storage trie of `hashed_address`.
    ///
    /// The targets are sorted in place and do not have to be sorted by the caller.
    fn storage_proof(
        &mut self,
        hashed_address: B256,
        targets: &mut [ProofV2Target],
    ) -> Result<Vec<ProofTrieNodeV2>, StateProofError>;
}
