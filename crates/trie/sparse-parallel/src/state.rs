//! Extensions for `SparseStateTrie` when using `ParallelSparseTrie`.

use crate::ParallelSparseTrie;
use alloy_primitives::B256;
use reth_trie_sparse::SparseStateTrie;

/// Extension trait for `SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie>` that provides
/// optimized proof target generation methods.
pub trait ParallelSparseStateTrieExt {
    /// Generate account proof targets using the parallel trie's optimized method.
    ///
    /// For each key, finds the deepest revealed node and returns a `(key, min_len)` pair.
    /// Keys that are already fully revealed (leaf exists) are excluded.
    fn generate_account_proof_targets_optimized(&self, keys: &[B256]) -> Vec<(B256, u8)>;

    /// Generate storage proof targets using the parallel trie's optimized method.
    ///
    /// For each storage key, finds the deepest revealed node and returns a `(key, min_len)` pair.
    /// Keys that are already fully revealed (leaf exists) are excluded.
    fn generate_storage_proof_targets_optimized(
        &self,
        account: B256,
        storage_keys: &[B256],
    ) -> Vec<(B256, u8)>;
}

impl ParallelSparseStateTrieExt for SparseStateTrie<ParallelSparseTrie, ParallelSparseTrie> {
    fn generate_account_proof_targets_optimized(&self, keys: &[B256]) -> Vec<(B256, u8)> {
        match self.state_trie_ref() {
            None => keys.iter().map(|key| (*key, 0u8)).collect(),
            Some(trie) => trie.generate_proof_targets(keys),
        }
    }

    fn generate_storage_proof_targets_optimized(
        &self,
        account: B256,
        storage_keys: &[B256],
    ) -> Vec<(B256, u8)> {
        match self.storage_trie_ref(&account) {
            None => storage_keys.iter().map(|key| (*key, 0u8)).collect(),
            Some(trie) => trie.generate_proof_targets(storage_keys),
        }
    }
}
