//! Results shared by storage proof workers and account value encoders.

use alloy_primitives::B256;
use reth_execution_errors::trie::StateProofError;
use reth_trie::ProofTrieNodeV2;

/// The results of a storage proof calculation.
#[derive(Debug)]
pub(crate) struct StorageProofResult {
    /// The calculated V2 proof nodes
    pub proof: Vec<ProofTrieNodeV2>,
    /// The storage root calculated by the V2 proof
    pub root: Option<B256>,
}

impl StorageProofResult {
    /// Returns the calculated root of the trie, if one can be calculated from the proof.
    pub(crate) const fn root(&self) -> Option<B256> {
        self.root
    }
}

/// Message containing a completed storage proof result with metadata.
#[derive(Debug)]
pub struct StorageProofResultMessage {
    /// The hashed address this storage proof belongs to
    #[allow(dead_code)]
    pub(crate) hashed_address: B256,
    /// The storage proof calculation result
    pub(crate) result: Result<StorageProofResult, StateProofError>,
}
