use super::{Proof, StorageProof};
use crate::{hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory};
use alloy_primitives::{map::HashSet, B256};
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind};
use reth_trie_common::{prefix_set::TriePrefixSetsMut, MultiProofTargets, Nibbles};
use reth_trie_sparse::blinded::{
    pad_path_to_key, BlindedProvider, BlindedProviderFactory, RevealedNode,
};
use std::{sync::Arc, time::Instant};
use tracing::{enabled, trace, Level};

/// Factory for instantiating providers capable of retrieving blinded trie nodes via proofs.
#[derive(Debug, Clone)]
pub struct ProofBlindedProviderFactory<T, H> {
    /// The cursor factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// A set of prefix sets that have changes.
    prefix_sets: Arc<TriePrefixSetsMut>,
}

impl<T, H> ProofBlindedProviderFactory<T, H> {
    /// Create new proof-based blinded provider factory.
    pub const fn new(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        prefix_sets: Arc<TriePrefixSetsMut>,
    ) -> Self {
        Self { trie_cursor_factory, hashed_cursor_factory, prefix_sets }
    }
}

impl<T, H> BlindedProviderFactory for ProofBlindedProviderFactory<T, H>
where
    T: TrieCursorFactory + Clone + Send + Sync,
    H: HashedCursorFactory + Clone + Send + Sync,
{
    type AccountNodeProvider = ProofBlindedAccountProvider<T, H>;
    type StorageNodeProvider = ProofBlindedStorageProvider<T, H>;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        ProofBlindedAccountProvider {
            trie_cursor_factory: self.trie_cursor_factory.clone(),
            hashed_cursor_factory: self.hashed_cursor_factory.clone(),
            prefix_sets: self.prefix_sets.clone(),
        }
    }

    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider {
        ProofBlindedStorageProvider {
            trie_cursor_factory: self.trie_cursor_factory.clone(),
            hashed_cursor_factory: self.hashed_cursor_factory.clone(),
            prefix_sets: self.prefix_sets.clone(),
            account,
        }
    }
}

/// Blinded provider for retrieving account trie nodes by path.
#[derive(Debug)]
pub struct ProofBlindedAccountProvider<T, H> {
    /// The cursor factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// A set of prefix sets that have changes.
    prefix_sets: Arc<TriePrefixSetsMut>,
}

impl<T, H> ProofBlindedAccountProvider<T, H> {
    /// Create new proof-based blinded account node provider.
    pub const fn new(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        prefix_sets: Arc<TriePrefixSetsMut>,
    ) -> Self {
        Self { trie_cursor_factory, hashed_cursor_factory, prefix_sets }
    }
}

impl<T, H> BlindedProvider for ProofBlindedAccountProvider<T, H>
where
    T: TrieCursorFactory + Clone + Send + Sync,
    H: HashedCursorFactory + Clone + Send + Sync,
{
    fn blinded_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let start = enabled!(target: "trie::proof::blinded", Level::TRACE).then(Instant::now);

        let targets = MultiProofTargets::from_iter([(pad_path_to_key(path), HashSet::default())]);
        let mut proof =
            Proof::new(self.trie_cursor_factory.clone(), self.hashed_cursor_factory.clone())
                .with_prefix_sets_mut(self.prefix_sets.as_ref().clone())
                .with_branch_node_masks(true)
                .multiproof(targets)
                .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
        let node = proof.account_subtree.into_inner().remove(path);
        let tree_mask = proof.branch_node_tree_masks.remove(path);
        let hash_mask = proof.branch_node_hash_masks.remove(path);

        trace!(
            target: "trie::proof::blinded",
            elapsed = ?start.unwrap().elapsed(),
            ?path,
            ?node,
            ?tree_mask,
            ?hash_mask,
            "Blinded node for account trie"
        );
        Ok(node.map(|node| RevealedNode { node, tree_mask, hash_mask }))
    }
}

/// Blinded provider for retrieving storage trie nodes by path.
#[derive(Debug)]
pub struct ProofBlindedStorageProvider<T, H> {
    /// The cursor factory for traversing trie nodes.
    trie_cursor_factory: T,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
    /// A set of prefix sets that have changes.
    prefix_sets: Arc<TriePrefixSetsMut>,
    /// Target account.
    account: B256,
}

impl<T, H> ProofBlindedStorageProvider<T, H> {
    /// Create new proof-based blinded storage node provider.
    pub const fn new(
        trie_cursor_factory: T,
        hashed_cursor_factory: H,
        prefix_sets: Arc<TriePrefixSetsMut>,
        account: B256,
    ) -> Self {
        Self { trie_cursor_factory, hashed_cursor_factory, prefix_sets, account }
    }
}

impl<T, H> BlindedProvider for ProofBlindedStorageProvider<T, H>
where
    T: TrieCursorFactory + Clone + Send + Sync,
    H: HashedCursorFactory + Clone + Send + Sync,
{
    fn blinded_node(&self, path: &Nibbles) -> Result<Option<RevealedNode>, SparseTrieError> {
        let start = enabled!(target: "trie::proof::blinded", Level::TRACE).then(Instant::now);

        let targets = HashSet::from_iter([pad_path_to_key(path)]);
        let storage_prefix_set =
            self.prefix_sets.storage_prefix_sets.get(&self.account).cloned().unwrap_or_default();
        let mut proof = StorageProof::new_hashed(
            self.trie_cursor_factory.clone(),
            self.hashed_cursor_factory.clone(),
            self.account,
        )
        .with_prefix_set_mut(storage_prefix_set)
        .with_branch_node_masks(true)
        .storage_multiproof(targets)
        .map_err(|error| SparseTrieErrorKind::Other(Box::new(error)))?;
        let node = proof.subtree.into_inner().remove(path);
        let tree_mask = proof.branch_node_tree_masks.remove(path);
        let hash_mask = proof.branch_node_hash_masks.remove(path);

        trace!(
            target: "trie::proof::blinded",
            account = ?self.account,
            elapsed = ?start.unwrap().elapsed(),
            ?path,
            ?node,
            ?tree_mask,
            ?hash_mask,
            "Blinded node for storage trie"
        );
        Ok(node.map(|node| RevealedNode { node, tree_mask, hash_mask }))
    }
}
