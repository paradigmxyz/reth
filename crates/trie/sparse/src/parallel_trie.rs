use crate::{blinded::BlindedProvider, SparseNode, SparseTrieUpdates, TrieMasks};
use alloc::vec::Vec;
use alloy_primitives::{map::HashMap, B256};
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{
    prefix_set::{PrefixSet, PrefixSetMut},
    Nibbles, TrieNode,
};
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

    /// Returns a list of [subtries](SparseSubtrie) identifying the subtries that have changed
    /// according to the provided [prefix set](PrefixSet).
    ///
    /// Along with the subtries, prefix sets are returned. Each prefix set contains the keys from
    /// the original prefix set that belong to the subtrie.
    ///
    /// This method helps optimize hash recalculations by identifying which specific
    /// subtries need to be updated. Each subtrie can then be updated in parallel.
    #[allow(unused)]
    fn get_changed_subtries(
        &mut self,
        prefix_set: &mut PrefixSet,
    ) -> Vec<(SparseSubtrie, PrefixSet)> {
        // Clone the prefix set to iterate over its keys. Cloning is cheap, it's just an Arc.
        let prefix_set_clone = prefix_set.clone();
        let mut prefix_set_iter = prefix_set_clone.into_iter();

        let mut subtries = Vec::new();
        for subtrie in &mut self.subtries {
            if let Some(subtrie) = subtrie.take_if(|subtrie| prefix_set.contains(&subtrie.path)) {
                let prefix_set = if prefix_set.all() {
                    PrefixSetMut::all()
                } else {
                    // Take those keys from the original prefix set that start with the subtrie path
                    //
                    // Subtries are stored in the order of their paths, so we can use the same
                    // prefix set iterator.
                    PrefixSetMut::from(
                        prefix_set_iter
                            .by_ref()
                            .skip_while(|key| key < &&subtrie.path)
                            .take_while(|key| key.starts_with(&subtrie.path))
                            .copied(),
                    )
                }
                .freeze();

                subtries.push((subtrie, prefix_set));
            }
        }
        subtries
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
    /// Creates a new sparse subtrie with the given root path.
    pub fn new(path: Nibbles) -> Self {
        Self { path, ..Default::default() }
    }

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
            Self::Lower(path_subtrie_index_unchecked(path))
        }
    }
}

/// Convert first two nibbles of the path into a lower subtrie index in the range [0, 255].
///
/// # Panics
///
/// If the path is shorter than two nibbles.
fn path_subtrie_index_unchecked(path: &Nibbles) -> usize {
    (path.get_unchecked(0) << 4 | path.get_unchecked(1)) as usize
}

#[cfg(test)]
mod tests {
    use alloy_trie::Nibbles;
    use reth_trie_common::prefix_set::{PrefixSet, PrefixSetMut};

    use crate::{
        parallel_trie::{path_subtrie_index_unchecked, SparseSubtrieType},
        ParallelSparseTrie, SparseSubtrie,
    };

    #[test]
    fn test_get_changed_subtries_empty() {
        let mut trie = ParallelSparseTrie::default();
        let mut prefix_set = PrefixSet::default();

        let changed = trie.get_changed_subtries(&mut prefix_set);
        assert!(changed.is_empty());
    }

    #[test]
    fn test_get_changed_subtries() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0]));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0]));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0]));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.subtries[subtrie_2_index] = Some(subtrie_2.clone());
        trie.subtries[subtrie_3_index] = Some(subtrie_3);

        // Create a prefix set with the keys that match only the second subtrie
        let mut prefix_set = PrefixSetMut::from([
            // Doesn't match any subtries
            Nibbles::from_nibbles_unchecked([0x0]),
            // Match second subtrie
            Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x0]),
            Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x1, 0x0]),
            // Doesn't match any subtries
            Nibbles::from_nibbles_unchecked([0x2, 0x0, 0x0]),
        ])
        .freeze();

        // Second subtrie should be removed and returned
        let changed = trie.get_changed_subtries(&mut prefix_set);
        assert_eq!(
            changed
                .into_iter()
                .map(|(subtrie, prefix_set)| {
                    (subtrie, prefix_set.iter().copied().collect::<Vec<_>>())
                })
                .collect::<Vec<_>>(),
            vec![(
                subtrie_2,
                vec![
                    Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x0]),
                    Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x1, 0x0])
                ]
            )]
        );
        assert!(trie.subtries[subtrie_2_index].is_none());

        // First subtrie should remain unchanged
        assert_eq!(trie.subtries[subtrie_1_index], Some(subtrie_1));
    }

    #[test]
    fn test_get_changed_subtries_all() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0]));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0]));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0]));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.subtries[subtrie_2_index] = Some(subtrie_2.clone());
        trie.subtries[subtrie_3_index] = Some(subtrie_3.clone());

        // Create a prefix set that matches any key
        let mut prefix_set = PrefixSetMut::all().freeze();

        // All subtries should be removed and returned
        let changed = trie.get_changed_subtries(&mut prefix_set);
        assert_eq!(
            changed
                .into_iter()
                .map(|(subtrie, prefix_set)| { (subtrie, prefix_set.all()) })
                .collect::<Vec<_>>(),
            vec![(subtrie_1, true), (subtrie_2, true), (subtrie_3, true)]
        );
        assert!(trie.subtries.iter().all(Option::is_none));
    }

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
