//! Configured sparse trie enum for switching between serial and parallel implementations.

use alloy_primitives::B256;
use reth_trie::{BranchNodeMasks, Nibbles, ProofTrieNode, TrieNode};
use reth_trie_sparse::{
    errors::SparseTrieResult, provider::TrieNodeProvider, LeafLookup, LeafLookupError,
    SerialSparseTrie, SparseTrieInterface, SparseTrieUpdates,
};
use reth_trie_sparse_parallel::ParallelSparseTrie;
use std::borrow::Cow;

/// Enum for switching between serial and parallel sparse trie implementations.
///
/// This type allows runtime selection between different sparse trie implementations,
/// providing flexibility in choosing the appropriate implementation based on workload
/// characteristics.
#[derive(Debug, Clone)]
pub(crate) enum ConfiguredSparseTrie {
    /// Serial implementation of the sparse trie.
    Serial(Box<SerialSparseTrie>),
    /// Parallel implementation of the sparse trie.
    Parallel(Box<ParallelSparseTrie>),
}

impl From<SerialSparseTrie> for ConfiguredSparseTrie {
    fn from(trie: SerialSparseTrie) -> Self {
        Self::Serial(Box::new(trie))
    }
}

impl From<ParallelSparseTrie> for ConfiguredSparseTrie {
    fn from(trie: ParallelSparseTrie) -> Self {
        Self::Parallel(Box::new(trie))
    }
}

impl Default for ConfiguredSparseTrie {
    fn default() -> Self {
        Self::Serial(Default::default())
    }
}

impl SparseTrieInterface for ConfiguredSparseTrie {
    fn with_root(
        self,
        root: TrieNode,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        match self {
            Self::Serial(trie) => {
                trie.with_root(root, masks, retain_updates).map(|t| Self::Serial(Box::new(t)))
            }
            Self::Parallel(trie) => {
                trie.with_root(root, masks, retain_updates).map(|t| Self::Parallel(Box::new(t)))
            }
        }
    }

    fn with_updates(self, retain_updates: bool) -> Self {
        match self {
            Self::Serial(trie) => Self::Serial(Box::new(trie.with_updates(retain_updates))),
            Self::Parallel(trie) => Self::Parallel(Box::new(trie.with_updates(retain_updates))),
        }
    }

    fn reserve_nodes(&mut self, additional: usize) {
        match self {
            Self::Serial(trie) => trie.reserve_nodes(additional),
            Self::Parallel(trie) => trie.reserve_nodes(additional),
        }
    }

    fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNode,
        masks: Option<BranchNodeMasks>,
    ) -> SparseTrieResult<()> {
        match self {
            Self::Serial(trie) => trie.reveal_node(path, node, masks),
            Self::Parallel(trie) => trie.reveal_node(path, node, masks),
        }
    }

    fn reveal_nodes(&mut self, nodes: Vec<ProofTrieNode>) -> SparseTrieResult<()> {
        match self {
            Self::Serial(trie) => trie.reveal_nodes(nodes),
            Self::Parallel(trie) => trie.reveal_nodes(nodes),
        }
    }

    fn update_leaf<P: TrieNodeProvider>(
        &mut self,
        full_path: Nibbles,
        value: Vec<u8>,
        provider: P,
    ) -> SparseTrieResult<()> {
        match self {
            Self::Serial(trie) => trie.update_leaf(full_path, value, provider),
            Self::Parallel(trie) => trie.update_leaf(full_path, value, provider),
        }
    }

    fn remove_leaf<P: TrieNodeProvider>(
        &mut self,
        full_path: &Nibbles,
        provider: P,
    ) -> SparseTrieResult<()> {
        match self {
            Self::Serial(trie) => trie.remove_leaf(full_path, provider),
            Self::Parallel(trie) => trie.remove_leaf(full_path, provider),
        }
    }

    fn root(&mut self) -> B256 {
        match self {
            Self::Serial(trie) => trie.root(),
            Self::Parallel(trie) => trie.root(),
        }
    }

    fn update_subtrie_hashes(&mut self) {
        match self {
            Self::Serial(trie) => trie.update_subtrie_hashes(),
            Self::Parallel(trie) => trie.update_subtrie_hashes(),
        }
    }

    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>> {
        match self {
            Self::Serial(trie) => trie.get_leaf_value(full_path),
            Self::Parallel(trie) => trie.get_leaf_value(full_path),
        }
    }

    fn find_leaf(
        &self,
        full_path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        match self {
            Self::Serial(trie) => trie.find_leaf(full_path, expected_value),
            Self::Parallel(trie) => trie.find_leaf(full_path, expected_value),
        }
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        match self {
            Self::Serial(trie) => trie.take_updates(),
            Self::Parallel(trie) => trie.take_updates(),
        }
    }

    fn wipe(&mut self) {
        match self {
            Self::Serial(trie) => trie.wipe(),
            Self::Parallel(trie) => trie.wipe(),
        }
    }

    fn clear(&mut self) {
        match self {
            Self::Serial(trie) => trie.clear(),
            Self::Parallel(trie) => trie.clear(),
        }
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        match self {
            Self::Serial(trie) => trie.updates_ref(),
            Self::Parallel(trie) => trie.updates_ref(),
        }
    }
    fn shrink_nodes_to(&mut self, size: usize) {
        match self {
            Self::Serial(trie) => trie.shrink_nodes_to(size),
            Self::Parallel(trie) => trie.shrink_nodes_to(size),
        }
    }

    fn shrink_values_to(&mut self, size: usize) {
        match self {
            Self::Serial(trie) => trie.shrink_values_to(size),
            Self::Parallel(trie) => trie.shrink_values_to(size),
        }
    }
}
