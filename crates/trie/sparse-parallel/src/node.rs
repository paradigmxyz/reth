use alloy_primitives::B256;
use alloy_trie::TrieMask;
use reth_trie_common::Nibbles;

/// Data stored for each leaf in the sparse trie.
///
/// This combines the short key (suffix), value, and cached hash that were previously
/// split between `SparseNode::Leaf` (in nodes map) and `values` map.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct LeafValue {
    /// The short key (suffix) for this leaf - the remaining nibbles after the prefix path.
    pub(crate) short_key: Nibbles,
    /// The RLP-encoded value stored at this leaf (account data or storage value).
    pub(crate) value: Vec<u8>,
    /// Cached hash of the leaf node's RLP encoding.
    /// Can be reused unless this leaf has been updated.
    pub(crate) hash: Option<B256>,
}

impl LeafValue {
    /// Creates a new leaf value with the given short key and value.
    pub(crate) const fn new(short_key: Nibbles, value: Vec<u8>) -> Self {
        Self { short_key, value, hash: None }
    }
}

/// Sparse trie node type for the parallel trie.
///
/// Unlike the upstream `SparseNode`, this type does **not** have a `Leaf` variant.
/// Leaves are stored externally in [`LeafValue`] via the `leaf_prefixes` / `values` maps
/// on `SparseSubtrieInner`, so structural nodes and leaf data are fully separated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum SparseNode {
    /// Empty trie node.
    Empty,
    /// The hash of the node that was not revealed.
    Hash(B256),
    /// Sparse extension node with key.
    Extension {
        /// The key slice stored by this extension node.
        key: Nibbles,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        store_in_db_trie: Option<bool>,
    },
    /// Sparse branch node with state mask.
    Branch {
        /// The bitmask representing children present in the branch node.
        state_mask: TrieMask,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        store_in_db_trie: Option<bool>,
    },
}

impl SparseNode {
    /// Create new [`SparseNode::Branch`] from state mask.
    pub(crate) const fn new_branch(state_mask: TrieMask) -> Self {
        Self::Branch { state_mask, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Branch`] with two bits set.
    pub(crate) const fn new_split_branch(bit_a: u8, bit_b: u8) -> Self {
        let state_mask = TrieMask::new((1u16 << bit_a) | (1u16 << bit_b));
        Self::Branch { state_mask, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Extension`] from the key slice.
    pub(crate) const fn new_ext(key: Nibbles) -> Self {
        Self::Extension { key, hash: None, store_in_db_trie: None }
    }

    /// Returns `true` if the node is a hash node.
    pub(crate) const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash(_))
    }

    /// Returns the hash of the node if it exists.
    pub(crate) const fn hash(&self) -> Option<B256> {
        match self {
            Self::Empty => None,
            Self::Hash(hash) => Some(*hash),
            Self::Extension { hash, .. } | Self::Branch { hash, .. } => *hash,
        }
    }

    /// Sets the hash of the node for testing purposes.
    ///
    /// For [`SparseNode::Empty`] and [`SparseNode::Hash`] nodes, this method does
    /// nothing.
    #[cfg(test)]
    pub(crate) const fn set_hash(&mut self, new_hash: Option<B256>) {
        match self {
            Self::Empty | Self::Hash(_) => {}
            Self::Extension { hash, .. } | Self::Branch { hash, .. } => {
                *hash = new_hash;
            }
        }
    }

    /// Returns the memory size of this node in bytes.
    pub(crate) const fn memory_size(&self) -> usize {
        match self {
            Self::Empty | Self::Hash(_) | Self::Branch { .. } => core::mem::size_of::<Self>(),
            Self::Extension { key, .. } => core::mem::size_of::<Self>() + key.len(),
        }
    }
}
