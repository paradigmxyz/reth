use crate::SparseTrie;
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
    #[allow(dead_code)]
    pub(crate) storages: HashMap<B256, SparseTrie>,
    /// Collection of revealed account and storage keys.
    #[allow(dead_code)]
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
        self.revealed.get(account).map_or(false, |slots| slots.contains(slot))
    }

    /// Reveal unknown trie paths from provided leaf path and its proof.
    ///
    /// # Panics
    ///
    /// This method panics on invalid proof if `debug_assertions` are enabled.
    /// However, it does not extensively validate the proof.
    pub fn reveal_account(
        &mut self,
        account: B256,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> alloy_rlp::Result<()> {
        let mut proof = proof.into_iter().peekable();

        // reveal root and initialize the trie of not already
        let Some((path, root)) = proof.next() else { return Ok(()) };
        debug_assert!(path.is_empty(), "first proof node is not root");
        let root_node = TrieNode::decode(&mut &root[..])?;
        debug_assert!(
            !matches!(root_node, TrieNode::EmptyRoot) || proof.peek().is_none(),
            "invalid proof"
        );
        let trie = self.state.reveal_root(root_node)?;

        // add the remaining proof nodes
        for (path, bytes) in proof {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node)?;
        }

        // Mark leaf path as revealed.
        self.revealed.entry(account).or_default();

        Ok(())
    }

    /// Returns sparse trie root if the the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        self.state.root()
    }

    /// Update the leaf node
    pub fn update_leaf(&mut self, path: Nibbles, value: Vec<u8>) {
        self.state.as_revealed_mut().unwrap().update_leaf(path, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_trie::HashBuilder;
    use reth_trie_common::proof::ProofRetainer;

    #[test]
    fn sparse_trie_reveal_empty() {
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

    #[cfg(debug_assertions)]
    mod debug_assertions {
        use super::*;
        use alloy_primitives::Bytes;
        use alloy_rlp::EMPTY_STRING_CODE;

        #[test]
        #[should_panic]
        fn reveal_first_node_not_root() {
            let mut sparse = SparseStateTrie::default();
            let proof = [(Nibbles::from_nibbles([0x1]), Bytes::from([EMPTY_STRING_CODE]))];
            sparse.reveal_account(Default::default(), proof).unwrap();
        }

        #[test]
        #[should_panic]
        fn reveal_invalid_proof_with_empty_root() {
            let mut sparse = SparseStateTrie::default();
            let proof = [
                (Nibbles::default(), Bytes::from([EMPTY_STRING_CODE])),
                (Nibbles::from_nibbles([0x1]), Bytes::new()),
            ];
            sparse.reveal_account(Default::default(), proof).unwrap();
        }
    }
}
