use crate::{SparseNode, SparseTrieUpdates};
use alloc::vec::Vec;
use alloy_primitives::map::HashMap;
use reth_trie_common::Nibbles;

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
