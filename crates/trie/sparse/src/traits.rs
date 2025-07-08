//! Traits for sparse trie implementations.

use core::fmt::Debug;

use alloc::vec::Vec;
use alloy_primitives::{
    map::{HashMap, HashSet},
    B256,
};
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{Nibbles, TrieNode};

use crate::blinded::BlindedProvider;

/// Trait defining common operations for revealed sparse trie implementations.
///
/// This trait abstracts over different sparse trie implementations (serial vs parallel)
/// while providing a unified interface for the core trie operations needed by the
/// [`crate::SparseTrie`] enum.
pub trait SparseTrieInterface: Default + Debug + Send + Sync {
    /// Creates a new revealed sparse trie from the given root node.
    ///
    /// This function initializes the internal structures and then reveals the root.
    /// It is a convenient method to create a trie when you already have the root node available.
    ///
    /// # Arguments
    ///
    /// * `root` - The root node of the trie
    /// * `masks` - Trie masks for root branch node
    /// * `retain_updates` - Whether to track updates
    ///
    /// # Returns
    ///
    /// Self if successful, or an error if revealing fails.
    fn from_root(root: TrieNode, masks: TrieMasks, retain_updates: bool) -> SparseTrieResult<Self>;

    /// Configures the trie to have the given root node revealed.
    ///
    /// # Arguments
    ///
    /// * `root` - The root node to reveal
    /// * `masks` - Trie masks for root branch node
    /// * `retain_updates` - Whether to track updates
    ///
    /// # Returns
    ///
    /// Self if successful, or an error if revealing fails.
    ///
    /// # Panics
    ///
    /// May panic if the trie is not new/cleared, and has already revealed nodes.
    fn with_root(
        self,
        root: TrieNode,
        masks: TrieMasks,
        retain_updates: bool,
    ) -> SparseTrieResult<Self>;

    /// Configures the trie to retain information about updates.
    ///
    /// If `retain_updates` is true, the trie will record branch node updates
    /// and deletions. This information can be used to efficiently update
    /// an external database.
    ///
    /// # Arguments
    ///
    /// * `retain_updates` - Whether to track updates
    ///
    /// # Returns
    ///
    /// Self for method chaining.
    fn with_updates(self, retain_updates: bool) -> Self;

    /// Reserves capacity for additional trie nodes.
    ///
    /// # Arguments
    ///
    /// * `additional` - The number of additional trie nodes to reserve capacity for.
    fn reserve_nodes(&mut self, _additional: usize) {}

    /// Reveals a trie node if it has not been revealed before.
    ///
    /// This function decodes a trie node and inserts it into the trie structure.
    /// It handles different node types (leaf, extension, branch) by appropriately
    /// adding them to the trie and recursively revealing their children.
    ///
    /// # Arguments
    ///
    /// * `path` - The path where the node should be revealed
    /// * `node` - The trie node to reveal
    /// * `masks` - Trie masks for branch nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the node was not revealed.
    fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNode,
        masks: TrieMasks,
    ) -> SparseTrieResult<()>;

    /// Updates the value of a leaf node at the specified path.
    ///
    /// If the leaf doesn't exist, it will be created.
    /// If it does exist, its value will be updated.
    ///
    /// # Arguments
    ///
    /// * `full_path` - The full path to the leaf
    /// * `value` - The new value for the leaf
    /// * `provider` - The blinded provider for resolving missing nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the update failed.
    fn update_leaf<P: BlindedProvider>(
        &mut self,
        full_path: Nibbles,
        value: Vec<u8>,
        provider: P,
    ) -> SparseTrieResult<()>;

    /// Removes a leaf node at the specified path.
    ///
    /// This will also handle collapsing the trie structure as needed
    /// (e.g., removing branch nodes that become unnecessary).
    ///
    /// # Arguments
    ///
    /// * `full_path` - The full path to the leaf to remove
    /// * `provider` - The blinded provider for resolving missing nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the removal failed.
    fn remove_leaf<P: BlindedProvider>(
        &mut self,
        full_path: &Nibbles,
        provider: P,
    ) -> SparseTrieResult<()>;

    /// Calculates and returns the root hash of the trie.
    ///
    /// This processes any dirty nodes by updating their RLP encodings
    /// and returns the root hash.
    ///
    /// # Returns
    ///
    /// The root hash of the trie.
    fn root(&mut self) -> B256;

    /// Recalculates and updates the RLP hashes of subtries deeper than a certain level. The level
    /// is defined in the implementation.
    ///
    /// The root node is considered to be at level 0. This method is useful for optimizing
    /// hash recalculations after localized changes to the trie structure.
    fn update_subtrie_hashes(&mut self);

    /// Retrieves a reference to the leaf value at the specified path.
    ///
    /// # Arguments
    ///
    /// * `full_path` - The full path to the leaf value
    ///
    /// # Returns
    ///
    /// A reference to the leaf value stored at the given full path, if it is revealed.
    ///
    /// Note: a value can exist in the full trie and this function still returns `None`
    /// because the value has not been revealed.
    ///
    /// Hence a `None` indicates two possibilities:
    /// - The value does not exists in the trie, so it cannot be revealed
    /// - The value has not yet been revealed. In order to determine which is true, one would need
    ///   an exclusion proof.
    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>>;

    /// Attempts to find a leaf node at the specified path.
    ///
    /// This method traverses the trie from the root down to the given path, checking
    /// if a leaf exists at that path. It can be used to verify the existence of a leaf
    /// or to generate an exclusion proof (proof that a leaf does not exist).
    ///
    /// # Parameters
    ///
    /// - `full_path`: The path to search for.
    /// - `expected_value`: Optional expected value. If provided, will verify the leaf value
    ///   matches.
    ///
    /// # Returns
    ///
    /// - `Ok(LeafLookup::Exists)` if the leaf exists with the expected value.
    /// - `Ok(LeafLookup::NonExistent)` if the leaf definitely does not exist (exclusion proof).
    /// - `Err(LeafLookupError)` if the search encountered a blinded node or found a different
    ///   value.
    fn find_leaf(
        &self,
        full_path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError>;

    /// Consumes and returns the currently accumulated trie updates.
    ///
    /// This is useful when you want to apply the updates to an external database
    /// and then start tracking a new set of updates.
    ///
    /// # Returns
    ///
    /// The accumulated updates, or an empty set if updates weren't being tracked.
    fn take_updates(&mut self) -> SparseTrieUpdates;

    /// Removes all nodes and values from the trie, resetting it to a blank state
    /// with only an empty root node. This is used when a storage root is deleted.
    ///
    /// This should not be used when intending to reuse the trie for a fresh account/storage root;
    /// use `clear` for that.
    ///
    /// Note: All previously tracked changes to the trie are also removed.
    fn wipe(&mut self);

    /// This clears all data structures in the sparse trie, keeping the backing data structures
    /// allocated. A [`crate::SparseNode::Empty`] is inserted at the root.
    ///
    /// This is useful for reusing the trie without needing to reallocate memory.
    fn clear(&mut self);
}

/// Struct for passing around branch node mask information.
///
/// Branch nodes can have up to 16 children (one for each nibble).
/// The masks represent which children are stored in different ways:
/// - `hash_mask`: Indicates which children are stored as hashes in the database
/// - `tree_mask`: Indicates which children are complete subtrees stored in the database
///
/// These masks are essential for efficient trie traversal and serialization, as they
/// determine how nodes should be encoded and stored on disk.
#[derive(Debug)]
pub struct TrieMasks {
    /// Branch node hash mask, if any.
    ///
    /// When a bit is set, the corresponding child node's hash is stored in the trie.
    ///
    /// This mask enables selective hashing of child nodes.
    pub hash_mask: Option<TrieMask>,
    /// Branch node tree mask, if any.
    ///
    /// When a bit is set, the corresponding child subtree is stored in the database.
    pub tree_mask: Option<TrieMask>,
}

impl TrieMasks {
    /// Helper function, returns both fields `hash_mask` and `tree_mask` as [`None`]
    pub const fn none() -> Self {
        Self { hash_mask: None, tree_mask: None }
    }
}

/// Tracks modifications to the sparse trie structure.
///
/// Maintains references to both modified and pruned/removed branches, enabling
/// one to make batch updates to a persistent database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SparseTrieUpdates {
    /// Collection of updated intermediate nodes indexed by full path.
    pub updated_nodes: HashMap<Nibbles, BranchNodeCompact>,
    /// Collection of removed intermediate nodes indexed by full path.
    pub removed_nodes: HashSet<Nibbles>,
    /// Flag indicating whether the trie was wiped.
    pub wiped: bool,
}

/// Error type for a leaf lookup operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafLookupError {
    /// The path leads to a blinded node, cannot determine if leaf exists.
    /// This means the witness is not complete.
    BlindedNode {
        /// Path to the blinded node.
        path: Nibbles,
        /// Hash of the blinded node.
        hash: B256,
    },
    /// The path leads to a leaf with a different value than expected.
    /// This means the witness is malformed.
    ValueMismatch {
        /// Path to the leaf.
        path: Nibbles,
        /// Expected value.
        expected: Option<Vec<u8>>,
        /// Actual value found.
        actual: Vec<u8>,
    },
}

/// Success value for a leaf lookup operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafLookup {
    /// Leaf exists with expected value.
    Exists,
    /// Leaf does not exist (exclusion proof found).
    NonExistent {
        /// Path where the search diverged from the target path.
        diverged_at: Nibbles,
    },
}
