//! Traits for sparse trie implementations.

use core::fmt::Debug;

use alloc::{borrow::Cow, vec::Vec};
use alloy_primitives::{
    map::{B256Map, HashMap, HashSet},
    B256,
};
use alloy_trie::BranchNodeCompact;
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, TrieNodeV2};

#[cfg(feature = "trie-debug")]
use crate::debug_recorder::TrieDebugRecorder;
use crate::provider::TrieNodeProvider;

/// Describes an update to a leaf in the sparse trie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafUpdate {
    /// The leaf value has been changed to the given RLP-encoded value.
    /// Empty Vec indicates the leaf has been removed.
    Changed(Vec<u8>),
    /// The leaf value may have changed, but the new value is not yet known.
    /// Used for optimistic prewarming when the actual value is unavailable.
    Touched,
}

impl LeafUpdate {
    /// Returns true if the leaf update is a change.
    pub const fn is_changed(&self) -> bool {
        matches!(self, Self::Changed(_))
    }

    /// Returns true if the leaf update is a touched update.
    pub const fn is_touched(&self) -> bool {
        matches!(self, Self::Touched)
    }
}

/// Trait defining common operations for revealed sparse trie implementations.
///
/// This trait abstracts over different sparse trie implementations (serial vs parallel)
/// while providing a unified interface for the core trie operations needed by the
/// [`crate::RevealableSparseTrie`] enum.
pub trait SparseTrie: Sized + Debug + Send + Sync {
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
    /// `Ok(())` if successful, or an error if revealing fails.
    ///
    /// # Panics
    ///
    /// May panic if the trie is not new/cleared, and has already revealed nodes.
    fn set_root(
        &mut self,
        root: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<()>;

    /// Configures the trie to have the given root node revealed.
    ///
    /// See [`Self::set_root`] for more details.
    fn with_root(
        mut self,
        root: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        self.set_root(root, masks, retain_updates)?;
        Ok(self)
    }

    /// Configures the trie to retain information about updates.
    ///
    /// If `retain_updates` is true, the trie will record branch node updates
    /// and deletions. This information can be used to efficiently update
    /// an external database.
    ///
    /// # Arguments
    ///
    /// * `retain_updates` - Whether to track updates
    fn set_updates(&mut self, retain_updates: bool);

    /// Configures the trie to retain information about updates.
    ///
    /// See [`Self::set_updates`] for more details.
    fn with_updates(mut self, retain_updates: bool) -> Self {
        self.set_updates(retain_updates);
        self
    }

    /// Reserves capacity for additional trie nodes.
    ///
    /// # Arguments
    ///
    /// * `additional` - The number of additional trie nodes to reserve capacity for.
    fn reserve_nodes(&mut self, _additional: usize) {}

    /// The single-node version of `reveal_nodes`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the node was not revealed.
    fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
    ) -> SparseTrieResult<()> {
        self.reveal_nodes(&mut [ProofTrieNodeV2 { path, node, masks }])
    }

    /// Reveals one or more trie nodes if they have not been revealed before.
    ///
    /// This function decodes trie nodes and inserts them into the trie structure. It handles
    /// different node types (leaf, extension, branch) by appropriately adding them to the trie and
    /// recursively revealing their children.
    ///
    /// # Arguments
    ///
    /// * `nodes` - The nodes to be revealed, each having a path and optional set of branch node
    ///   masks. The nodes will be unsorted.
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if any of the nodes was not revealed.
    ///
    /// # Note
    ///
    /// The implementation may modify the input nodes. A common thing to do is [`std::mem::replace`]
    /// each node with [`TrieNodeV2::EmptyRoot`] to avoid cloning.
    fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()>;

    /// Updates the value of a leaf node at the specified path.
    ///
    /// If the leaf doesn't exist, it will be created.
    /// If it does exist, its value will be updated.
    ///
    /// # Arguments
    ///
    /// * `full_path` - The full path to the leaf
    /// * `value` - The new value for the leaf
    /// * `provider` - The trie provider for resolving missing nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the update failed.
    fn update_leaf<P: TrieNodeProvider>(
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
    /// * `provider` - The trie node provider for resolving missing nodes
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the removal failed.
    fn remove_leaf<P: TrieNodeProvider>(
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

    /// Returns true if the root node is cached and does not need any recomputation.
    fn is_root_cached(&self) -> bool;

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

    /// Returns a reference to the current sparse trie updates.
    ///
    /// If no updates have been made/recorded, returns an empty update set.
    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates>;

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

    /// Shrink the capacity of the sparse trie's node storage to the given size.
    /// This will reduce memory usage if the current capacity is higher than the given size.
    fn shrink_nodes_to(&mut self, size: usize);

    /// Shrink the capacity of the sparse trie's value storage to the given size.
    /// This will reduce memory usage if the current capacity is higher than the given size.
    fn shrink_values_to(&mut self, size: usize);

    /// Returns a cheap O(1) size hint for the trie representing the count of revealed
    /// (non-Hash) nodes.
    ///
    /// This is used as a heuristic for prioritizing which storage tries to keep
    /// during pruning. Larger values indicate larger tries that are more valuable to preserve.
    fn size_hint(&self) -> usize;

    /// Replaces nodes beyond `max_depth` with hash stubs and removes their descendants.
    ///
    /// Depth counts nodes traversed (not nibbles), so extension nodes count as 1 depth
    /// regardless of key length. `max_depth == 0` prunes all children of the root node.
    ///
    /// # Preconditions
    ///
    /// Must be called after `root()` to ensure all nodes have computed hashes.
    /// Calling on a trie without computed hashes will result in no pruning.
    ///
    /// # Behavior
    ///
    /// - Embedded nodes (RLP < 32 bytes) are preserved since they have no hash
    /// - Returns 0 if `max_depth` exceeds trie depth or trie is empty
    ///
    /// # Returns
    ///
    /// The number of nodes converted to hash stubs.
    fn prune(&mut self, max_depth: usize) -> usize;

    /// Takes the debug recorder out of this trie, replacing it with an empty one.
    ///
    /// Returns the recorder containing all recorded mutations since the last reset.
    /// The default implementation returns an empty recorder.
    #[cfg(feature = "trie-debug")]
    fn take_debug_recorder(&mut self) -> TrieDebugRecorder {
        TrieDebugRecorder::default()
    }

    /// Applies leaf updates to the sparse trie.
    ///
    /// When a [`LeafUpdate::Changed`] is successfully applied, it is removed from the
    /// given [`B256Map`]. If it could not be applied due to blinded nodes, it remains
    /// in the map and the callback is invoked with the required proof target.
    ///
    /// Once that proof is calculated and revealed via [`SparseTrie::reveal_nodes`], the same
    /// `updates` map can be reused to retry the update.
    ///
    /// The callback receives `(key, min_len)` where `key` is the full 32-byte hashed key
    /// (right-padded with zeros from the blinded path) and `min_len` is the minimum depth
    /// at which proof nodes should be returned.
    ///
    /// The callback may be invoked multiple times for the same target across retry loops.
    /// Callers should deduplicate if needed.
    ///
    /// [`LeafUpdate::Touched`] behaves identically except it does not modify the leaf value.
    fn update_leaves(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()>;

    /// Commits the updated nodes to internal trie state.
    fn commit_updates(
        &mut self,
        updated: &HashMap<Nibbles, BranchNodeCompact>,
        removed: &HashSet<Nibbles>,
    );
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

impl SparseTrieUpdates {
    /// Initialize a [`Self`] with given capacities.
    pub fn with_capacity(num_updated_nodes: usize, num_removed_nodes: usize) -> Self {
        Self {
            updated_nodes: HashMap::with_capacity_and_hasher(num_updated_nodes, Default::default()),
            removed_nodes: HashSet::with_capacity_and_hasher(num_removed_nodes, Default::default()),
            wiped: false,
        }
    }
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
    NonExistent,
}
