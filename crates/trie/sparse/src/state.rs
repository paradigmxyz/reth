use std::iter::Peekable;

use crate::{SparseStateTrieError, SparseStateTrieResult, SparseTrie};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Bytes, B256,
};
use alloy_rlp::Decodable;
use reth_trie::{Nibbles, TrieNode};

/// Sparse state trie representing lazy-loaded Ethereum state trie.
#[derive(Default, Debug)]
pub struct SparseStateTrie {
    /// Sparse account trie.
    pub(crate) state: SparseTrie,
    /// Sparse storage tries.
    pub(crate) storages: HashMap<B256, SparseTrie>,
    /// Collection of revealed account and storage keys.
    pub(crate) revealed: HashMap<B256, HashSet<B256>>,
}

impl SparseStateTrie {
    /// Create state trie from state trie.
    pub fn from_state(state: SparseTrie) -> Self {
        Self { state, ..Default::default() }
    }

    /// Returns `true` if account was already revealed.
    pub fn is_account_revealed(&self, account: &B256) -> bool {
        self.revealed.contains_key(account)
    }

    /// Returns `true` if storage slot for account was already revealed.
    pub fn is_storage_slot_revealed(&self, account: &B256, slot: &B256) -> bool {
        self.revealed.get(account).is_some_and(|slots| slots.contains(slot))
    }

    /// Reveal unknown trie paths from provided leaf path and its proof for the account.
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_account(
        &mut self,
        account: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> SparseStateTrieResult<()> {
        if self.revealed.contains_key(&account) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_proof(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.state.reveal_root(root_node)?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default();

        Ok(())
    }

    /// Reveal unknown trie paths from provided leaf path and its proof for the storage slot.
    /// NOTE: This method does not extensively validate the proof.
    pub fn reveal_storage_slot(
        &mut self,
        account: B256,
        slot: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> SparseStateTrieResult<()> {
        if self.revealed.get(&account).is_some_and(|v| v.contains(&slot)) {
            return Ok(());
        }

        let mut proof = proof.into_iter().peekable();

        let Some(root_node) = self.validate_proof(&mut proof)? else { return Ok(()) };

        // Reveal root node if it wasn't already.
        let trie = self.storages.entry(account).or_default().reveal_root(root_node)?;

        // Reveal the remaining proof nodes.
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default().insert(slot);

        Ok(())
    }

    /// Validates the root node of the proof and returns it if it exists and is valid.
    fn validate_proof<I: Iterator<Item = (Nibbles, Bytes)>>(
        &self,
        proof: &mut Peekable<I>,
    ) -> SparseStateTrieResult<Option<TrieNode>> {
        let mut proof = proof.into_iter().peekable();

        // Validate root node.
        let Some((path, node)) = proof.next() else { return Ok(None) };
        if !path.is_empty() {
            return Err(SparseStateTrieError::InvalidRootNode { path, node })
        }

        // Decode root node and perform sanity check.
        let root_node = TrieNode::decode(&mut &node[..])?;
        if matches!(root_node, TrieNode::EmptyRoot) && proof.peek().is_some() {
            return Err(SparseStateTrieError::InvalidRootNode { path, node })
        }

        Ok(Some(root_node))
    }

    /// Update the leaf node.
    pub fn update_leaf(&mut self, path: Nibbles, value: Vec<u8>) -> SparseStateTrieResult<()> {
        self.state.update_leaf(path, value)?;
        Ok(())
    }

    /// Returns sparse trie root if the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        self.state.root()
    }

    /// Returns storage sparse trie root if the trie has been revealed.
    pub fn storage_root(&mut self, account: B256) -> Option<B256> {
        self.storages.get_mut(&account).and_then(|trie| trie.root())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use alloy_rlp::EMPTY_STRING_CODE;
    use assert_matches::assert_matches;
    use reth_trie::HashBuilder;
    use reth_trie_common::proof::ProofRetainer;

    #[test]
    fn validate_proof_first_node_not_root() {
        let sparse = SparseStateTrie::default();
        let proof = [(Nibbles::from_nibbles([0x1]), Bytes::from([EMPTY_STRING_CODE]))];
        assert_matches!(
            sparse.validate_proof(&mut proof.into_iter().peekable()),
            Err(SparseStateTrieError::InvalidRootNode { .. })
        );
    }

    #[test]
    fn validate_proof_invalid_proof_with_empty_root() {
        let sparse = SparseStateTrie::default();
        let proof = [
            (Nibbles::default(), Bytes::from([EMPTY_STRING_CODE])),
            (Nibbles::from_nibbles([0x1]), Bytes::new()),
        ];
        assert_matches!(
            sparse.validate_proof(&mut proof.into_iter().peekable()),
            Err(SparseStateTrieError::InvalidRootNode { .. })
        );
    }

    #[test]
    fn reveal_account_empty() {
        let retainer = ProofRetainer::from_iter([Nibbles::default()]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        hash_builder.root();
        let proofs = hash_builder.take_proof_nodes();
        assert_eq!(proofs.len(), 1);

        let mut sparse = SparseStateTrie::default();
        assert_eq!(sparse.state, SparseTrie::Blind);
        sparse.reveal_account(Default::default(), proofs.into_inner()).unwrap();
        assert_eq!(sparse.state, SparseTrie::revealed_empty());
    }

    #[test]
    fn reveal_storage_slot_empty() {
        let retainer = ProofRetainer::from_iter([Nibbles::default()]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        hash_builder.root();
        let proofs = hash_builder.take_proof_nodes();
        assert_eq!(proofs.len(), 1);

        let mut sparse = SparseStateTrie::default();
        assert!(sparse.storages.is_empty());
        sparse
            .reveal_storage_slot(Default::default(), Default::default(), proofs.into_inner())
            .unwrap();
        assert_eq!(
            sparse.storages,
            HashMap::from_iter([(Default::default(), SparseTrie::revealed_empty())])
        );
    }
}
