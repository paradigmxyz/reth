use crate::{blinded::BlindedProvider, SparseNode, SparseTrieUpdates, TrieMasks};
use alloc::vec::Vec;
use alloy_primitives::{map::HashMap, B256};
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{prefix_set::PrefixSet, Nibbles, TrieNode};
use tracing::trace;

/// A revealed sparse trie with subtries that can be updated in parallel.
///
/// ## Invariants
///
/// - Each leaf entry in the `subtries` and `upper_trie` collection must have a corresponding entry
///   in `values` collection. If the root node is a leaf, it must also have an entry in `values`.
/// - All keys in `values` collection are full leaf paths.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParallelSparseTrie {
    /// This contains the trie nodes for the upper part of the trie.
    upper_subtrie: SparseSubtrie,
    /// An array containing the subtries at the second level of the trie.
    subtries: [Option<SparseSubtrie>; 256],
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            upper_subtrie: SparseSubtrie::default(),
            subtries: [const { None }; 256],
            updates: None,
        }
    }
}

impl ParallelSparseTrie {
    /// Creates a new revealed sparse trie from the given root node.
    ///
    /// # Returns
    ///
    /// A [`ParallelSparseTrie`] if successful, or an error if revealing fails.
    pub fn from_root(
        root_node: TrieNode,
        masks: TrieMasks,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        let mut trie = Self::default().with_updates(retain_updates);
        trie.reveal_node(Nibbles::default(), root_node, masks)?;
        Ok(trie)
    }

    /// Reveals a trie node if it has not been revealed before.
    ///
    /// This internal function decodes a trie node and inserts it into the nodes map.
    /// It handles different node types (leaf, extension, branch) by appropriately
    /// adding them to the trie structure and recursively revealing their children.
    ///
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if node was not revealed.
    pub fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNode,
        masks: TrieMasks,
    ) -> SparseTrieResult<()> {
        let _path = path;
        let _node = node;
        let _masks = masks;
        todo!()
    }

    /// Updates or inserts a leaf node at the specified key path with the provided RLP-encoded
    /// value.
    ///
    /// This method updates the internal prefix set and, if the leaf did not previously exist,
    /// adjusts the trie structure by inserting new leaf nodes, splitting branch nodes, or
    /// collapsing extension nodes as needed.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the update is successful.
    ///
    /// Note: If an update requires revealing a blinded node, an error is returned if the blinded
    /// provider returns an error.
    pub fn update_leaf(
        &mut self,
        key_path: Nibbles,
        value: Vec<u8>,
        masks: TrieMasks,
        provider: impl BlindedProvider,
    ) -> SparseTrieResult<()> {
        let _key_path = key_path;
        let _value = value;
        let _masks = masks;
        let _provider = provider;
        todo!()
    }

    /// Removes a leaf node from the trie at the specified key path.
    ///
    /// This function removes the leaf value from the internal values map and then traverses
    /// the trie to remove or adjust intermediate nodes, merging or collapsing them as necessary.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the leaf is successfully removed, otherwise returns an error
    /// if the leaf is not present or if a blinded node prevents removal.
    pub fn remove_leaf(
        &mut self,
        path: &Nibbles,
        provider: impl BlindedProvider,
    ) -> SparseTrieResult<()> {
        let _path = path;
        let _provider = provider;
        todo!()
    }

    /// Recalculates and updates the RLP hashes of nodes up to level 2 of the trie.
    ///
    /// The root node is considered to be at level 0. This method is useful for optimizing
    /// hash recalculations after localized changes to the trie structure.
    ///
    /// This function first identifies all nodes that have changed (based on the prefix set) below
    /// level 2 of the trie, then recalculates their RLP representation.
    pub fn update_subtrie_hashes(&mut self) -> SparseTrieResult<()> {
        trace!(target: "trie::parallel_sparse", "Updating subtrie hashes");
        todo!()
    }

    /// Calculates and returns the root hash of the trie.
    ///
    /// Before computing the hash, this function processes any remaining (dirty) nodes by
    /// updating their RLP encodings. The root hash is either:
    /// 1. The cached hash (if no dirty nodes were found)
    /// 2. The keccak256 hash of the root node's RLP representation
    pub fn root(&mut self) -> B256 {
        trace!(target: "trie::parallel_sparse", "Calculating trie root hash");
        todo!()
    }

    /// Configures the trie to retain information about updates.
    ///
    /// If `retain_updates` is true, the trie will record branch node updates and deletions.
    /// This information can then be used to efficiently update an external database.
    pub fn with_updates(mut self, retain_updates: bool) -> Self {
        if retain_updates {
            self.updates = Some(SparseTrieUpdates::default());
        }
        self
    }
}

/// This is a subtrie of the [`ParallelSparseTrie`] that contains a map from path to sparse trie
/// nodes.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SparseSubtrie {
    /// The root path of this subtrie.
    ///
    /// This is the _full_ path to this subtrie, meaning it includes the first two nibbles that we
    /// also use for indexing subtries in the [`ParallelSparseTrie`].
    path: Nibbles,
    /// The map from paths to sparse trie nodes within this subtrie.
    nodes: HashMap<Nibbles, SparseNode>,
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
}

impl SparseSubtrie {
    /// Recalculates and updates the RLP hashes for the changed nodes in this subtrie.
    pub fn update_hashes(&mut self, prefix_set: &mut PrefixSet) -> SparseTrieResult<()> {
        trace!(target: "trie::parallel_sparse", path=?self.path, "Updating subtrie hashes");
        let _prefix_set = prefix_set;
        todo!()
    }
}

/// Sparse Subtrie Type.
///
/// Used to determine the type of subtrie a certain path belongs to:
/// - Paths in the range `0x..=0xff` belong to the upper subtrie.
/// - Paths in the range `0x000..` belong to one of the lower subtries. The index of the lower
///   subtrie is determined by the path first nibbles of the path.
///
/// There can be at most 256 lower subtries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SparseSubtrieType {
    /// Upper subtrie with paths in the range `0x..=0xff`
    Upper,
    /// Lower subtrie with paths in the range `0x000..`. Includes the index of the subtrie,
    /// according to the path prefix.
    Lower(usize),
}

impl SparseSubtrieType {
    /// Returns the type of subtrie based on the given path.
    pub fn from_path(path: &Nibbles) -> Self {
        if path.len() <= 2 {
            Self::Upper
        } else {
            // Convert first two nibbles of the path into a number.
            let index = (path[0] << 4 | path[1]) as usize;
            Self::Lower(index)
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_trie::Nibbles;

    use crate::parallel_trie::SparseSubtrieType;

    #[test]
    fn sparse_subtrie_type() {
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 0])),
            SparseSubtrieType::Upper
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 15])),
            SparseSubtrieType::Upper
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 0, 0])),
            SparseSubtrieType::Lower(0)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 1, 0])),
            SparseSubtrieType::Lower(1)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 15, 0])),
            SparseSubtrieType::Lower(15)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 0, 0])),
            SparseSubtrieType::Lower(240)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 1, 0])),
            SparseSubtrieType::Lower(241)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 15, 0])),
            SparseSubtrieType::Lower(255)
        );
    }
}
