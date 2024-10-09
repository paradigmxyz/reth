use crate::SparseTrie;
use alloy_primitives::{map::HashMap, Bytes, B256};
use alloy_rlp::Decodable;
use reth_trie::{Nibbles, TrieNode};

/// TODO: WIP
#[derive(Default, Debug)]
pub struct SparseStateTrie {
    /// Sparse account trie.
    pub(crate) state: SparseTrie,
    /// Sparse storage tries.
    storages: HashMap<B256, SparseTrie>,
}

impl SparseStateTrie {
    /// Create state trie from state trie.
    pub fn from_state(state: SparseTrie) -> Self {
        Self { state, ..Default::default() }
    }

    /// Reveal unknown trie paths from provided leaf path and its proof.
    ///
    /// # Panics
    ///
    /// This method panics on invalid proof if `debug_assertions` are enabled.
    /// However, it does not extensively validate the proof.
    pub fn reveal(
        &mut self,
        _leaf_path: Nibbles,
        proof: impl IntoIterator<Item = (Nibbles, Bytes)>,
    ) -> alloy_rlp::Result<()> {
        let mut proof = proof.into_iter().peekable();

        // reveal root and initialize the trie of not already
        let Some((path, root)) = proof.next() else { return Ok(()) };
        debug_assert!(path.is_empty(), "first proof node is not root");
        let root_node = TrieNode::decode(&mut &root[..])?;
        debug_assert!(!root_node.is_empty_root() || proof.peek().is_none(), "invalid proof");
        let trie = self.state.reveal_root(root_node)?;

        // add the remaining proof nodes
        while let Some((path, bytes)) = proof.next() {
            let node = TrieNode::decode(&mut &bytes[..])?;
            trie.reveal_node(path, node)?;
        }

        Ok(())
    }

    /// Returns sparse trie root if the the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        self.state.root()
    }

    /// Update the leaf node
    pub fn update_leaf(&mut self, path: Nibbles, value: Vec<u8>) {
        // TODO: self.updated_leaves.insert(key, value);
        self.state.as_revealed_mut().unwrap().update_leaf(path, value);
    }
}
