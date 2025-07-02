//! Traits for sparse trie implementations.

use alloc::vec::Vec;
use alloy_primitives::B256;
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{Nibbles, TrieNode};

use crate::{
    blinded::BlindedProvider,
    SparseTrieUpdates, TrieMasks,
};

/// Trait defining common operations for revealed sparse trie implementations.
///
/// This trait abstracts over different sparse trie implementations (serial vs parallel)
/// while providing a unified interface for the core trie operations needed by the
/// SparseTrie enum.
pub trait SparseTrieInterface: Default + Clone + PartialEq + Eq + core::fmt::Debug {
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
    /// * `path` - The full path to the leaf
    /// * `value` - The new value for the leaf
    /// * `masks` - Trie masks for branch nodes (may be ignored by some implementations)
    /// * `provider` - The blinded provider for resolving missing nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the update failed.
    fn update_leaf<P: BlindedProvider>(
        &mut self,
        path: Nibbles,
        value: Vec<u8>,
        masks: TrieMasks,
        provider: P,
    ) -> SparseTrieResult<()>;

    /// Removes a leaf node at the specified path.
    ///
    /// This will also handle collapsing the trie structure as needed
    /// (e.g., removing branch nodes that become unnecessary).
    ///
    /// # Arguments
    ///
    /// * `path` - The full path to the leaf to remove
    /// * `provider` - The blinded provider for resolving missing nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the removal failed.
    fn remove_leaf<P: BlindedProvider>(&mut self, path: &Nibbles, provider: P) -> SparseTrieResult<()>;

    /// Calculates and returns the root hash of the trie.
    ///
    /// This processes any dirty nodes by updating their RLP encodings
    /// and returns the root hash.
    ///
    /// # Returns
    ///
    /// The root hash of the trie.
    fn root(&mut self) -> B256;

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

    /// Consumes and returns the currently accumulated trie updates.
    ///
    /// This is useful when you want to apply the updates to an external database
    /// and then start tracking a new set of updates.
    ///
    /// # Returns
    ///
    /// The accumulated updates, or an empty set if updates weren't being tracked.
    fn take_updates(&mut self) -> SparseTrieUpdates;
}