use crate::{
    ArenaParallelSparseTrie, LeafUpdate, SparseTrie as SparseTrieTrait, SparseTrieUpdates,
    TrieNodeEpoch,
};
use alloc::boxed::Box;
use alloy_primitives::{map::B256Map, B256};
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{BranchNodeMasks, ProofTrieNodeV2, ProofV2TargetParent, TrieNodeV2};

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
pub enum RevealableSparseTrie<T = ArenaParallelSparseTrie> {
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
    /// Creates a new revealed but empty sparse trie.
    pub fn revealed_empty() -> Self {
        Self::Revealed(Box::default())
    }

    /// Reveals the root node, converting a blind trie into a revealed one.
    ///
    /// If the trie is blinded, its root node is replaced with `root`.
    ///
    /// The `masks` are used to determine how the node's children are stored.
    /// The retention flag controls whether trie updates should be tracked.
    ///
    /// # Returns
    ///
    /// A mutable reference to the underlying [`RevealableSparseTrie`](SparseTrieTrait).
    pub fn reveal_root(
        &mut self,
        root: TrieNodeV2,
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

    /// Reveals a batch of V2 proof nodes into this trie.
    ///
    /// If `nodes` contains a node at the empty path it is used to reveal the root (transitioning
    /// the trie from blind to revealed). Otherwise the trie must already be revealed.
    pub fn reveal_v2_proof_nodes(
        &mut self,
        nodes: &mut [ProofTrieNodeV2],
        retain_updates: bool,
    ) -> SparseTrieResult<()> {
        let trie = if let Some(root_node) = nodes.iter().find(|n| n.path.is_empty()) {
            self.reveal_root(root_node.node.clone(), root_node.masks, retain_updates)?
        } else {
            self.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?
        };
        trie.reveal_nodes(nodes)?;

        Ok(())
    }
}

impl<T: SparseTrieTrait> RevealableSparseTrie<T> {
    /// Creates a new blind sparse trie.
    ///
    /// # Examples
    ///
    /// ```
    /// use reth_trie_sparse::RevealableSparseTrie;
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
    pub fn root(&mut self, new_epoch: TrieNodeEpoch) -> Option<B256> {
        Some(self.as_revealed_mut()?.root(new_epoch))
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
    pub fn root_with_updates(
        &mut self,
        new_epoch: TrieNodeEpoch,
    ) -> Option<(B256, SparseTrieUpdates)> {
        let revealed = self.as_revealed_mut()?;
        Some((revealed.root(new_epoch), revealed.take_updates()))
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
}

impl<T: SparseTrieTrait + Default> RevealableSparseTrie<T> {
    /// Applies batch leaf updates to the sparse trie.
    ///
    /// For blind tries, all updates are kept in the map and proof targets are emitted
    /// for every key (with no known parent since nothing is revealed).
    ///
    /// For revealed tries, delegates to the inner implementation which will:
    /// - Apply updates where possible
    /// - Keep blocked updates in the map
    /// - Emit proof targets for blinded paths
    pub fn update_leaves(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        mut proof_required_fn: impl FnMut(B256, ProofV2TargetParent),
    ) -> SparseTrieResult<()> {
        match self {
            Self::Blind(_) => {
                // Nothing is revealed - emit proof targets for all keys without a known parent.
                for key in updates.keys() {
                    proof_required_fn(*key, ProofV2TargetParent::NONE);
                }
                // All updates remain in the map for retry after proofs are fetched
                Ok(())
            }
            Self::Revealed(trie) => trie.update_leaves(updates, proof_required_fn),
        }
    }
}

impl SparseTrieUpdates {
    /// Clears the updates, but keeps the backing data structures allocated.
    pub fn clear(&mut self) {
        self.updated_nodes.clear();
        self.removed_nodes.clear();
    }

    /// Extends the updates with another set of updates.
    pub fn extend(&mut self, other: Self) {
        self.updated_nodes.extend(other.updated_nodes);
        self.removed_nodes.extend(other.removed_nodes);
    }
}
