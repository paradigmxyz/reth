use crate::{SparseNode, SparseTrieUpdates, TrieMasks};
use alloc::vec::Vec;
use alloy_primitives::map::HashMap;
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{prefix_set::PrefixSet, Nibbles, TrieNode};

/// A revealed sparse trie with subtries that can be updated in parallel.
///
/// ## Invariants
///
/// - Each leaf entry in the `subtries` and `upper_trie` collection must have a corresponding entry
///   in `values` collection. If the root node is a leaf, it must also have an entry in `values`.
/// - All keys in `values` collection are full leaf paths.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParallelSparseTrie {
    /// The root of the sparse trie.
    root_node: SparseNode,
    /// Map from a path (nibbles) to its corresponding sparse trie node.
    /// This contains the trie nodes for the upper part of the trie.
    upper_trie: HashMap<Nibbles, SparseNode>,
    /// An array containing the subtries at the second level of the trie.
    subtries: [Option<SparseSubtrie>; 256],
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            root_node: SparseNode::Empty,
            upper_trie: HashMap::default(),
            subtries: [const { None }; 256],
            values: HashMap::default(),
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

    /// Returns a list of [`SparseSubtrie`] identifying the subtries that have changed according to
    /// the provided [prefix set](PrefixSet).
    ///
    /// This method helps optimize hash recalculations by identifying which specific
    /// subtries need to be updated. Each subtrie can then be updated in parallel.
    #[allow(unused)]
    fn get_changed_subtries(&mut self, prefix_set: &mut PrefixSet) -> Vec<SparseSubtrie> {
        self.subtries
            .iter_mut()
            .filter_map(|subtrie| subtrie.take_if(|subtrie| prefix_set.contains(&subtrie.path)))
            .collect()
    }
}

/// This is a subtrie of the `ParallelSparseTrie` that contains a map from path to sparse trie
/// nodes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SparseSubtrie {
    /// The root path of this subtrie.
    path: Nibbles,
    /// The map from paths to sparse trie nodes within this subtrie.
    nodes: HashMap<Nibbles, SparseNode>,
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
    (path[0] << 4 | path[1]) as usize
}

#[cfg(test)]
mod tests {
    use alloy_primitives::map::HashMap;
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
        // Create a trie with two subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 =
            SparseSubtrie { path: Nibbles::from_nibbles([0x0, 0x0]), nodes: HashMap::default() };
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 =
            SparseSubtrie { path: Nibbles::from_nibbles([0x1, 0x0]), nodes: HashMap::default() };
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);

        // Add subtries at specific positions
        trie.subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.subtries[subtrie_2_index] = Some(subtrie_2.clone());

        // Create a prefix set that matches only the second subtrie
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([0x1, 0x0, 0x1])); // Matches second subtrie
        let mut prefix_set = prefix_set_mut.freeze();

        // Second subtrie should be removed and returned
        let changed = trie.get_changed_subtries(&mut prefix_set);
        assert_eq!(changed, vec![subtrie_2]);
        assert!(trie.subtries[subtrie_2_index].is_none());

        // First subtrie should remain unchanged
        assert_eq!(trie.subtries[subtrie_1_index], Some(subtrie_1));
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
