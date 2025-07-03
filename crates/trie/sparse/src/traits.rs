//! Traits for sparse trie implementations.

use core::fmt::Debug;

use alloc::vec::Vec;
use alloy_primitives::B256;
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{Nibbles, TrieNode};

use crate::{blinded::BlindedProvider, SparseTrieUpdates, TrieMasks};

/// Trait defining common operations for revealed sparse trie implementations.
///
/// This trait abstracts over different sparse trie implementations (serial vs parallel)
/// while providing a unified interface for the core trie operations needed by the
/// [`crate::SparseTrie`] enum.
pub trait SparseTrieInterface: Default + Debug {
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
    fn reserve_nodes(&mut self, additional: usize);

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
    /// This should not be used when intending to re-use the trie for a fresh account/storage root;
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
