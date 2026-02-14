use crate::{
    provider::TrieNodeProvider, LeafUpdate, ParallelSparseTrie, SparseTrie as SparseTrieTrait,
    SparseTrieUpdates,
};
use alloc::{borrow::Cow, boxed::Box, vec, vec::Vec};
use alloy_primitives::{map::B256Map, B256};
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{BranchNodeMasks, Nibbles, RlpNode, TrieMask, TrieNode};
use tracing::instrument;

/// A sparse trie that is either in a "blind" state (no nodes are revealed, root node hash is
/// unknown) or in a "revealed" state (root node has been revealed and the trie can be updated).
///
/// In blind mode the trie does not contain any decoded node data, which saves memory but
/// prevents direct access to node contents. The revealed mode stores decoded nodes along
/// with additional information such as values, allowing direct manipulation.
///
/// The sparse trie design is optimised for:
/// 1. Memory efficiency - only revealed nodes are loaded into memory
/// 2. Update tracking - changes to the trie structure can be tracked and selectively persisted
/// 3. Incremental operations - nodes can be revealed as needed without loading the entire trie.
///    This is what gives rise to the notion of a "sparse" trie.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum RevealableSparseTrie<T = ParallelSparseTrie> {
    /// The trie is blind -- no nodes have been revealed
    ///
    /// This is the default state. In this state, the trie cannot be directly queried or modified
    /// until nodes are revealed.
    ///
    /// In this state the `RevealableSparseTrie` can optionally carry with it a cleared
    /// sparse trie. This allows for reusing the trie's allocations between payload executions.
    Blind(Option<Box<T>>),
    /// Some nodes in the Trie have been revealed.
    ///
    /// In this state, the trie can be queried and modified for the parts
    /// that have been revealed. Other parts remain blind and require revealing
    /// before they can be accessed.
    Revealed(Box<T>),
}

impl<T: Default> Default for RevealableSparseTrie<T> {
    fn default() -> Self {
        Self::Blind(None)
    }
}

impl<T: SparseTrieTrait + Default> RevealableSparseTrie<T> {
    /// Creates a new revealed but empty sparse trie with `SparseNode::Empty` as root node.
    pub fn revealed_empty() -> Self {
        Self::Revealed(Box::default())
    }

    /// Reveals the root node, converting a blind trie into a revealed one.
    ///
    /// If the trie is blinded, its root node is replaced with `root`.
    ///
    /// The `masks` are used to determine how the node's children are stored.
    /// The `retain_updates` flag controls whether changes to the trie structure
    /// should be tracked.
    ///
    /// # Returns
    ///
    /// A mutable reference to the underlying [`RevealableSparseTrie`](SparseTrieTrait).
    pub fn reveal_root(
        &mut self,
        root: TrieNode,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<&mut T> {
        // if `Blind`, we initialize the revealed trie with the given root node, using a
        // pre-allocated trie if available.
        if self.is_blind() {
            let mut revealed_trie = if let Self::Blind(Some(cleared_trie)) = core::mem::take(self) {
                cleared_trie
            } else {
                Box::default()
            };

            revealed_trie.set_root(root, masks, retain_updates)?;
            *self = Self::Revealed(revealed_trie);
        }

        Ok(self.as_revealed_mut().unwrap())
    }
}

impl<T: SparseTrieTrait> RevealableSparseTrie<T> {
    /// Creates a new blind sparse trie.
    ///
    /// # Examples
    ///
    /// ```
    /// use reth_trie_sparse::{provider::DefaultTrieNodeProvider, RevealableSparseTrie};
    ///
    /// let trie = <RevealableSparseTrie>::blind();
    /// assert!(trie.is_blind());
    /// let trie = <RevealableSparseTrie>::default();
    /// assert!(trie.is_blind());
    /// ```
    pub const fn blind() -> Self {
        Self::Blind(None)
    }

    /// Creates a new blind sparse trie, clearing and later reusing the given
    /// [`RevealableSparseTrie`](SparseTrieTrait).
    pub fn blind_from(mut trie: T) -> Self {
        trie.clear();
        Self::Blind(Some(Box::new(trie)))
    }

    /// Returns `true` if the sparse trie has no revealed nodes.
    pub const fn is_blind(&self) -> bool {
        matches!(self, Self::Blind(_))
    }

    /// Returns `true` if the sparse trie is revealed.
    pub const fn is_revealed(&self) -> bool {
        matches!(self, Self::Revealed(_))
    }

    /// Returns an immutable reference to the underlying revealed sparse trie.
    ///
    /// Returns `None` if the trie is blinded.
    pub const fn as_revealed_ref(&self) -> Option<&T> {
        if let Self::Revealed(revealed) = self {
            Some(revealed)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying revealed sparse trie.
    ///
    /// Returns `None` if the trie is blinded.
    pub fn as_revealed_mut(&mut self) -> Option<&mut T> {
        if let Self::Revealed(revealed) = self {
            Some(revealed)
        } else {
            None
        }
    }

    /// Wipes the trie by removing all nodes and values,
    /// and resetting the trie to only contain an empty root node.
    ///
    /// Note: This method will error if the trie is blinded.
    pub fn wipe(&mut self) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?;
        revealed.wipe();
        Ok(())
    }

    /// Calculates the root hash of the trie.
    ///
    /// This will update any remaining dirty nodes before computing the root hash.
    /// "dirty" nodes are nodes that need their hashes to be recomputed because one or more of their
    /// children's hashes have changed.
    ///
    /// # Returns
    ///
    /// - `Some(B256)` with the calculated root hash if the trie is revealed.
    /// - `None` if the trie is still blind.
    pub fn root(&mut self) -> Option<B256> {
        Some(self.as_revealed_mut()?.root())
    }

    /// Returns true if the root node is cached and does not need any recomputation.
    pub fn is_root_cached(&self) -> bool {
        self.as_revealed_ref().is_some_and(|trie| trie.is_root_cached())
    }

    /// Returns the root hash along with any accumulated update information.
    ///
    /// This is useful for when you need both the root hash and information about
    /// what nodes were modified, which can be used to efficiently update
    /// an external database.
    ///
    /// # Returns
    ///
    /// An `Option` tuple consisting of:
    ///  - The trie root hash (`B256`).
    ///  - A [`SparseTrieUpdates`] structure containing information about updated nodes.
    ///  - `None` if the trie is still blind.
    pub fn root_with_updates(&mut self) -> Option<(B256, SparseTrieUpdates)> {
        let revealed = self.as_revealed_mut()?;
        Some((revealed.root(), revealed.take_updates()))
    }

    /// Clears this trie, setting it to a blind state.
    ///
    /// If this instance was revealed, or was itself a `Blind` with a pre-allocated
    /// [`RevealableSparseTrie`](SparseTrieTrait), this will set to `Blind` carrying a cleared
    /// pre-allocated [`RevealableSparseTrie`](SparseTrieTrait).
    #[inline]
    pub fn clear(&mut self) {
        *self = match core::mem::replace(self, Self::blind()) {
            s @ Self::Blind(_) => s,
            Self::Revealed(mut trie) => {
                trie.clear();
                Self::Blind(Some(trie))
            }
        };
    }

    /// Updates (or inserts) a leaf at the given key path with the specified RLP-encoded value.
    ///
    /// # Errors
    ///
    /// Returns an error if the trie is still blind, or if the update fails.
    #[instrument(level = "trace", target = "trie::sparse", skip_all)]
    pub fn update_leaf(
        &mut self,
        path: Nibbles,
        value: Vec<u8>,
        provider: impl TrieNodeProvider,
    ) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?;
        revealed.update_leaf(path, value, provider)?;
        Ok(())
    }

    /// Removes a leaf node at the specified key path.
    ///
    /// # Errors
    ///
    /// Returns an error if the trie is still blind, or if the leaf cannot be removed
    #[instrument(level = "trace", target = "trie::sparse", skip_all)]
    pub fn remove_leaf(
        &mut self,
        path: &Nibbles,
        provider: impl TrieNodeProvider,
    ) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?;
        revealed.remove_leaf(path, provider)?;
        Ok(())
    }

    /// Shrinks the capacity of the sparse trie's node storage.
    /// Works for both revealed and blind tries with allocated storage.
    pub fn shrink_nodes_to(&mut self, size: usize) {
        match self {
            Self::Blind(Some(trie)) | Self::Revealed(trie) => {
                trie.shrink_nodes_to(size);
            }
            _ => {}
        }
    }

    /// Shrinks the capacity of the sparse trie's value storage.
    /// Works for both revealed and blind tries with allocated storage.
    pub fn shrink_values_to(&mut self, size: usize) {
        match self {
            Self::Blind(Some(trie)) | Self::Revealed(trie) => {
                trie.shrink_values_to(size);
            }
            _ => {}
        }
    }
}

impl<T: SparseTrieTrait + Default> RevealableSparseTrie<T> {
    /// Applies batch leaf updates to the sparse trie.
    ///
    /// For blind tries, all updates are kept in the map and proof targets are emitted
    /// for every key (with `min_len = 0` since nothing is revealed).
    ///
    /// For revealed tries, delegates to the inner implementation which will:
    /// - Apply updates where possible
    /// - Keep blocked updates in the map
    /// - Emit proof targets for blinded paths
    pub fn update_leaves(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        mut proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()> {
        match self {
            Self::Blind(_) => {
                // Nothing is revealed - emit proof targets for all keys with min_len = 0
                for key in updates.keys() {
                    proof_required_fn(*key, 0);
                }
                // All updates remain in the map for retry after proofs are fetched
                Ok(())
            }
            Self::Revealed(trie) => trie.update_leaves(updates, proof_required_fn),
        }
    }
}

/// Enum representing sparse trie node type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparseNodeType {
    /// Empty trie node.
    Empty,
    /// A placeholder that stores only the hash for a node that has not been fully revealed.
    Hash,
    /// Sparse leaf node.
    Leaf,
    /// Sparse extension node.
    Extension {
        /// A flag indicating whether the extension node should be stored in the database.
        store_in_db_trie: Option<bool>,
    },
    /// Sparse branch node.
    Branch {
        /// A flag indicating whether the branch node should be stored in the database.
        store_in_db_trie: Option<bool>,
    },
}

impl SparseNodeType {
    /// Returns true if the node is a hash node.
    pub const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash)
    }

    /// Returns true if the node is a branch node.
    pub const fn is_branch(&self) -> bool {
        matches!(self, Self::Branch { .. })
    }

    /// Returns true if the node should be stored in the database.
    pub const fn store_in_db_trie(&self) -> Option<bool> {
        match *self {
            Self::Extension { store_in_db_trie } | Self::Branch { store_in_db_trie } => {
                store_in_db_trie
            }
            _ => None,
        }
    }
}

/// Enum representing trie nodes in sparse trie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparseNode {
    /// Empty trie node.
    Empty,
    /// The hash of the node that was not revealed.
    Hash(B256),
    /// A leaf node that is at the root of the trie (path is empty).
    /// This variant is used when the entire trie contains only a single key-value pair.
    RootLeaf {
        /// The full key for this leaf (since it's at the root, the key suffix equals the full
        /// key).
        key: Nibbles,
        /// Tracker for the node's state, e.g. cached `RlpNode` tracking.
        state: SparseNodeState,
    },
    /// Sparse extension node with key.
    Extension {
        /// The key slice stored by this extension node.
        key: Nibbles,
        /// Tracker for the node's state, e.g. cached `RlpNode` tracking.
        state: SparseNodeState,
    },
    /// Sparse branch node with state mask.
    Branch {
        /// The bitmask representing children present in the branch node.
        state_mask: TrieMask,
        /// The bitmask representing which children are leaf nodes.
        /// This is always a subset of `state_mask`.
        leaf_mask: TrieMask,
        /// Short keys (key suffixes) for leaf children, ordered by nibble index.
        /// The i-th element corresponds to the i-th set bit in `leaf_mask` (counting from LSB).
        /// Invariant: `leaf_short_keys.len() == leaf_mask.count_ones()`.
        leaf_short_keys: Vec<Nibbles>,
        /// Tracker for the node's state, e.g. cached `RlpNode` tracking.
        state: SparseNodeState,
    },
}

impl SparseNode {
    /// Create new [`SparseNode::Branch`] from state mask only.
    ///
    /// The leaf_mask is set to empty (no leaf children). Use this when all children are non-leaf
    /// nodes (branch, extension, or hash).
    #[inline]
    pub fn new_branch(state_mask: TrieMask) -> Self {
        Self::Branch {
            state_mask,
            leaf_mask: TrieMask::default(),
            leaf_short_keys: Vec::new(),
            state: SparseNodeState::Dirty,
        }
    }

    /// Create new [`SparseNode::Branch`] with two bits set, both pointing to leaf children.
    ///
    /// The short keys must be provided in ascending nibble order.
    /// - `bit_a` and `bit_b` are nibble indices for the children
    /// - `short_key_a` and `short_key_b` are the short keys for each leaf child
    #[inline]
    pub fn new_split_branch(
        bit_a: u8,
        short_key_a: Nibbles,
        bit_b: u8,
        short_key_b: Nibbles,
    ) -> Self {
        let state_mask = TrieMask::new((1u16 << bit_a) | (1u16 << bit_b));
        let leaf_mask = state_mask;
        let leaf_short_keys = if bit_a < bit_b {
            vec![short_key_a, short_key_b]
        } else {
            vec![short_key_b, short_key_a]
        };
        Self::Branch { state_mask, leaf_mask, leaf_short_keys, state: SparseNodeState::Dirty }
    }

    /// Create new [`SparseNode::Branch`] with two bits set, specifying which is a leaf.
    ///
    /// - `bit_a` and `bit_b` are nibble indices for the children
    /// - `short_key_a` is the short key if `bit_a` is a leaf, otherwise `None`
    /// - `short_key_b` is the short key if `bit_b` is a leaf, otherwise `None`
    #[inline]
    pub fn new_split_branch_with_keys(
        bit_a: u8,
        short_key_a: Option<Nibbles>,
        bit_b: u8,
        short_key_b: Option<Nibbles>,
    ) -> Self {
        let state_mask = TrieMask::new((1u16 << bit_a) | (1u16 << bit_b));
        let mut leaf_mask_val = 0u16;
        let mut leaf_short_keys = Vec::with_capacity(2);

        // Collect leaf keys in ascending nibble order
        let (first_bit, first_key, second_bit, second_key) = if bit_a < bit_b {
            (bit_a, short_key_a, bit_b, short_key_b)
        } else {
            (bit_b, short_key_b, bit_a, short_key_a)
        };

        if let Some(key) = first_key {
            leaf_mask_val |= 1u16 << first_bit;
            leaf_short_keys.push(key);
        }
        if let Some(key) = second_key {
            leaf_mask_val |= 1u16 << second_bit;
            leaf_short_keys.push(key);
        }

        let leaf_mask = TrieMask::new(leaf_mask_val);
        Self::Branch { state_mask, leaf_mask, leaf_short_keys, state: SparseNodeState::Dirty }
    }

    /// Create new [`SparseNode::Extension`] from the key slice.
    #[inline]
    pub const fn new_ext(key: Nibbles) -> Self {
        Self::Extension { key, state: SparseNodeState::Dirty }
    }

    /// Create new [`SparseNode::RootLeaf`] from leaf key.
    /// This should be used when the leaf is at the root of the trie (path is empty).
    #[inline]
    pub const fn new_root_leaf(key: Nibbles) -> Self {
        Self::RootLeaf { key, state: SparseNodeState::Dirty }
    }

    /// Returns `true` if the node is a hash node.
    #[inline]
    pub const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash(_))
    }

    /// Returns `true` if the node is a leaf node (either `RootLeaf`).
    #[inline]
    pub const fn is_leaf(&self) -> bool {
        matches!(self, Self::RootLeaf { .. })
    }

    /// Returns the cached [`RlpNode`] of the node, if it's available.
    pub fn cached_rlp_node(&self) -> Option<Cow<'_, RlpNode>> {
        match &self {
            Self::Empty => None,
            Self::Hash(hash) => Some(Cow::Owned(RlpNode::word_rlp(hash))),
            Self::RootLeaf { state, .. } |
            Self::Extension { state, .. } |
            Self::Branch { state, .. } => state.cached_rlp_node().map(Cow::Borrowed),
        }
    }

    /// Returns the cached hash of the node, if it's available.
    pub fn cached_hash(&self) -> Option<B256> {
        match &self {
            Self::Empty => None,
            Self::Hash(hash) => Some(*hash),
            Self::RootLeaf { state, .. } |
            Self::Extension { state, .. } |
            Self::Branch { state, .. } => state.cached_hash(),
        }
    }

    /// Sets the hash of the node for testing purposes.
    ///
    /// For [`SparseNode::Empty`] and [`SparseNode::Hash`] nodes, this method panics.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_state(&mut self, new_state: SparseNodeState) {
        match self {
            Self::Empty | Self::Hash(_) => {
                panic!("Cannot set hash for Empty or Hash nodes")
            }
            Self::RootLeaf { state, .. } |
            Self::Extension { state, .. } |
            Self::Branch { state, .. } => {
                *state = new_state;
            }
        }
    }

    /// Returns the memory size of this node in bytes.
    pub fn memory_size(&self) -> usize {
        match self {
            Self::Empty | Self::Hash(_) => core::mem::size_of::<Self>(),
            Self::RootLeaf { key, .. } | Self::Extension { key, .. } => {
                core::mem::size_of::<Self>() + key.len()
            }
            Self::Branch { leaf_short_keys, .. } => {
                let mut size = core::mem::size_of::<Self>();
                size += leaf_short_keys.capacity() * core::mem::size_of::<Nibbles>();
                for key in leaf_short_keys {
                    size += key.len();
                }
                size
            }
        }
    }

    /// Returns the rank (0-based index) of a nibble within the leaf_mask.
    ///
    /// The rank is the number of bits set in `leaf_mask` at positions less than `nibble`.
    /// This is used to index into `leaf_short_keys`.
    #[inline]
    pub fn leaf_rank(leaf_mask: TrieMask, nibble: u8) -> usize {
        debug_assert!(nibble < 16);
        let mask_below = (1u16 << nibble) - 1;
        (leaf_mask.get() & mask_below).count_ones() as usize
    }

    /// Gets the short key for a leaf child at the given nibble.
    ///
    /// Returns `None` if the nibble is not a leaf child (not set in `leaf_mask`).
    #[inline]
    pub fn get_leaf_short_key(&self, nibble: u8) -> Option<&Nibbles> {
        match self {
            Self::Branch { leaf_mask, leaf_short_keys, .. } => {
                if !leaf_mask.is_bit_set(nibble) {
                    return None;
                }
                let idx = Self::leaf_rank(*leaf_mask, nibble);
                leaf_short_keys.get(idx)
            }
            _ => None,
        }
    }

    /// Sets the short key for a leaf child at the given nibble.
    ///
    /// If the nibble is already a leaf, updates the short key.
    /// If the nibble is not a leaf (but is set in state_mask), adds it as a leaf.
    ///
    /// # Panics
    ///
    /// Panics if the node is not a Branch or if the nibble is not set in state_mask.
    #[inline]
    pub fn set_leaf_short_key(&mut self, nibble: u8, short_key: Nibbles) {
        match self {
            Self::Branch { state_mask, leaf_mask, leaf_short_keys, state } => {
                debug_assert!(state_mask.is_bit_set(nibble), "nibble must be set in state_mask");

                if leaf_mask.is_bit_set(nibble) {
                    // Update existing
                    let idx = Self::leaf_rank(*leaf_mask, nibble);
                    leaf_short_keys[idx] = short_key;
                } else {
                    // Insert new
                    leaf_mask.set_bit(nibble);
                    let idx = Self::leaf_rank(*leaf_mask, nibble);
                    leaf_short_keys.insert(idx, short_key);
                }
                *state = SparseNodeState::Dirty;
            }
            _ => panic!("set_leaf_short_key called on non-Branch node"),
        }
    }

    /// Removes the short key for a leaf child at the given nibble.
    ///
    /// Returns the removed short key, or `None` if the nibble was not a leaf.
    ///
    /// # Panics
    ///
    /// Panics if the node is not a Branch.
    #[inline]
    pub fn remove_leaf_short_key(&mut self, nibble: u8) -> Option<Nibbles> {
        match self {
            Self::Branch { leaf_mask, leaf_short_keys, state, .. } => {
                if !leaf_mask.is_bit_set(nibble) {
                    return None;
                }
                let idx = Self::leaf_rank(*leaf_mask, nibble);
                leaf_mask.unset_bit(nibble);
                *state = SparseNodeState::Dirty;
                Some(leaf_short_keys.remove(idx))
            }
            _ => panic!("remove_leaf_short_key called on non-Branch node"),
        }
    }

    /// Adds a new child to a branch node.
    ///
    /// If `short_key` is `Some`, the child is marked as a leaf.
    /// If `short_key` is `None`, the child is marked as a non-leaf.
    ///
    /// # Panics
    ///
    /// Panics if the node is not a Branch or if the nibble is already set in state_mask.
    #[inline]
    pub fn add_child(&mut self, nibble: u8, short_key: Option<Nibbles>) {
        match self {
            Self::Branch { state_mask, leaf_mask, leaf_short_keys, state } => {
                debug_assert!(
                    !state_mask.is_bit_set(nibble),
                    "nibble must not already be set in state_mask"
                );

                state_mask.set_bit(nibble);
                if let Some(key) = short_key {
                    leaf_mask.set_bit(nibble);
                    let idx = Self::leaf_rank(*leaf_mask, nibble);
                    leaf_short_keys.insert(idx, key);
                }
                *state = SparseNodeState::Dirty;
            }
            _ => panic!("add_child called on non-Branch node"),
        }
    }

    /// Removes a child from a branch node.
    ///
    /// Returns the short key if the child was a leaf, or `None` if it was not.
    ///
    /// # Panics
    ///
    /// Panics if the node is not a Branch or if the nibble is not set in state_mask.
    #[inline]
    pub fn remove_child(&mut self, nibble: u8) -> Option<Nibbles> {
        match self {
            Self::Branch { state_mask, leaf_mask, leaf_short_keys, state } => {
                debug_assert!(state_mask.is_bit_set(nibble), "nibble must be set in state_mask");

                state_mask.unset_bit(nibble);
                *state = SparseNodeState::Dirty;

                if leaf_mask.is_bit_set(nibble) {
                    let idx = Self::leaf_rank(*leaf_mask, nibble);
                    leaf_mask.unset_bit(nibble);
                    Some(leaf_short_keys.remove(idx))
                } else {
                    None
                }
            }
            _ => panic!("remove_child called on non-Branch node"),
        }
    }
}

/// Tracks the current state of a node in the trie, specifically regarding whether it's been updated
/// or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparseNodeState {
    /// The node has been updated and its new `RlpNode` has not yet been calculated.
    ///
    /// If a node is dirty and has children (branches or extensions) then at least once child must
    /// also be dirty.
    Dirty,
    /// The node has a cached `RlpNode`, either from being revealed or computed after an update.
    Cached {
        /// The RLP node which is used to represent this node in its parent. Usually this is the
        /// RLP encoding of the node's hash, except for when the node RLP encodes to <32
        /// bytes.
        rlp_node: RlpNode,
        /// Flag indicating if this node is cached in the database.
        ///
        /// NOTE for extension nodes this actually indicates the node's child branch is in the
        /// database, not the extension itself.
        store_in_db_trie: Option<bool>,
    },
}

impl SparseNodeState {
    /// Returns the cached [`RlpNode`] of the node, if it's available.
    pub const fn cached_rlp_node(&self) -> Option<&RlpNode> {
        match self {
            Self::Cached { rlp_node, .. } => Some(rlp_node),
            Self::Dirty => None,
        }
    }

    /// Returns the cached hash of the node, if it's available.
    pub fn cached_hash(&self) -> Option<B256> {
        self.cached_rlp_node().and_then(|n| n.as_hash())
    }

    /// Returns whether or not this node is stored in the db, or None if it's not known.
    pub const fn store_in_db_trie(&self) -> Option<bool> {
        match self {
            Self::Cached { store_in_db_trie, .. } => *store_in_db_trie,
            Self::Dirty => None,
        }
    }
}

/// A leaf node in the sparse trie, containing both metadata and value.
///
/// This struct combines the leaf's key suffix length, state tracking, and value
/// in a single allocation, avoiding the indirection of storing leaf metadata
/// separately from values.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SparseNodeLeaf {
    /// Length of the key suffix for this leaf.
    /// The actual key can be derived from the full path: `full_path[full_path.len() - key_len..]`
    pub key_len: usize,
    /// Tracker for the node's state, e.g. cached `RlpNode` tracking.
    pub state: SparseNodeState,
    /// The RLP-encoded value stored at this leaf.
    pub value: Vec<u8>,
}

impl SparseNodeLeaf {
    /// Creates a new leaf node with the given key length, value, and dirty state.
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(key_len: usize, value: Vec<u8>) -> Self {
        Self { key_len, state: SparseNodeState::Dirty, value }
    }

    /// Returns the key suffix for this leaf given the full path.
    pub fn key(&self, full_path: &Nibbles) -> Nibbles {
        full_path.slice(full_path.len() - self.key_len..)
    }

    /// Returns the cached [`RlpNode`] of the leaf, if it's available.
    pub const fn cached_rlp_node(&self) -> Option<&RlpNode> {
        self.state.cached_rlp_node()
    }

    /// Returns the cached hash of the leaf, if it's available.
    pub fn cached_hash(&self) -> Option<B256> {
        self.state.cached_hash()
    }

    /// Returns the memory size of this leaf in bytes.
    #[allow(clippy::missing_const_for_fn)]
    pub fn memory_size(&self) -> usize {
        core::mem::size_of::<Self>() + self.value.len()
    }
}

/// RLP node stack item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RlpNodeStackItem {
    /// Path to the node.
    pub path: Nibbles,
    /// RLP node.
    pub rlp_node: RlpNode,
    /// Type of the node.
    pub node_type: SparseNodeType,
}

impl SparseTrieUpdates {
    /// Create new wiped sparse trie updates.
    pub fn wiped() -> Self {
        Self { wiped: true, ..Default::default() }
    }

    /// Clears the updates, but keeps the backing data structures allocated.
    ///
    /// Sets `wiped` to `false`.
    pub fn clear(&mut self) {
        self.updated_nodes.clear();
        self.removed_nodes.clear();
        self.wiped = false;
    }

    /// Extends the updates with another set of updates.
    pub fn extend(&mut self, other: Self) {
        self.updated_nodes.extend(other.updated_nodes);
        self.removed_nodes.extend(other.removed_nodes);
        self.wiped |= other.wiped;
    }
}
