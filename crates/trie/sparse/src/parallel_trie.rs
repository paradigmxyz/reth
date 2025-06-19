use crate::{
    blinded::BlindedProvider, RlpNodePathStackItem, RlpNodeStackItem, SparseNode, SparseNodeType,
    SparseTrieUpdates, TrieMasks,
};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::{map::HashMap, B256};
use alloy_trie::{BranchNodeCompact, TrieMask, EMPTY_ROOT_HASH};
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{
    prefix_set::{PrefixSet, PrefixSetMut},
    BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, RlpNode, TrieNode, CHILD_INDEX_RANGE,
};
use smallvec::SmallVec;
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
    lower_subtries: Box<[Option<SparseSubtrie>; 256]>,
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            upper_subtrie: SparseSubtrie::default(),
            lower_subtries: Box::new([const { None }; 256]),
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
        for subtrie in self.lower_subtries.iter_mut() {
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
                            .take_while(|key| key.has_prefix(&subtrie.path))
                            .cloned(),
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
    ///
    /// There should be a node for this path in `nodes` map.
    path: Nibbles,
    /// The map from paths to sparse trie nodes within this subtrie.
    nodes: HashMap<Nibbles, SparseNode>,
    /// When a branch is set, the corresponding child subtree is stored in the database.
    branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
    /// When a bit is set, the corresponding child is stored as a hash in the database.
    branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
    /// Whether to retain [`SparseTrieUpdates`].
    retain_updates: bool,
    buffers: SparseSubtrieBuffers,
}

impl SparseSubtrie {
    /// Creates a new sparse subtrie with the given root path.
    pub fn new(path: Nibbles) -> Self {
        Self { path, ..Default::default() }
    }

    /// Configures the trie to retain information about updates.
    ///
    /// If `retain_updates` is true, the trie will record branch node updates and deletions.
    /// This information can then be used to efficiently update an external database.
    pub const fn with_updates(mut self, retain_updates: bool) -> Self {
        self.retain_updates = retain_updates;
        self
    }

    /// Recalculates and updates the RLP hashes for the changed nodes in this subtrie.
    ///
    /// The function starts from the subtrie root, traverses down to leaves, and then calculates
    /// the hashes from leaves back up to the root. It uses a stack from [`RlpNodeBuffers`] to
    /// track the traversal and accumulate RLP encodings.
    ///
    /// # Parameters
    ///
    /// - `prefix_set`: The set of trie paths whose nodes have changed.
    ///
    /// # Returns
    ///
    /// A tuple containing the root node of the updated subtrie and an optional set of updates.
    /// Updates are [`Some`] if [`Self::with_updates`] was set to `true`.
    ///
    /// # Panics
    ///
    /// If the node at the root path does not exist.
    pub fn update_hashes(
        &mut self,
        prefix_set: &mut PrefixSet,
    ) -> (RlpNode, Option<SparseTrieUpdates>) {
        trace!(target: "trie::parallel_sparse", root=?self.path, "Updating subtrie hashes");

        debug_assert!(self.buffers.path_stack.is_empty());
        self.buffers.path_stack.push(RlpNodePathStackItem {
            level: 0,
            path: self.path.clone(),
            is_in_prefix_set: None,
        });

        let mut updates = self.retain_updates.then_some(SparseTrieUpdates::default());

        'main: while let Some(RlpNodePathStackItem { level, path, mut is_in_prefix_set }) =
            self.buffers.path_stack.pop()
        {
            let node = self.nodes.get_mut(&path).unwrap();
            trace!(
                target: "trie::parallel_sparse",
                root = ?self.path,
                ?level,
                ?path,
                ?is_in_prefix_set,
                ?node,
                "Popped node from path stack"
            );

            // Check if the path is in the prefix set.
            // First, check the cached value. If it's `None`, then check the prefix set, and update
            // the cached value.
            let mut prefix_set_contains =
                |path: &Nibbles| *is_in_prefix_set.get_or_insert_with(|| prefix_set.contains(path));

            let (rlp_node, node_type) = match node {
                SparseNode::Empty => (RlpNode::word_rlp(&EMPTY_ROOT_HASH), SparseNodeType::Empty),
                SparseNode::Hash(hash) => (RlpNode::word_rlp(hash), SparseNodeType::Hash),
                SparseNode::Leaf { key, hash } => {
                    let mut path = path.clone();
                    path.extend_from_slice_unchecked(key);
                    if let Some(hash) = hash.filter(|_| !prefix_set_contains(&path)) {
                        (RlpNode::word_rlp(&hash), SparseNodeType::Leaf)
                    } else {
                        let value = self.values.get(&path).unwrap();
                        self.buffers.rlp_buf.clear();
                        let rlp_node = LeafNodeRef { key, value }.rlp(&mut self.buffers.rlp_buf);
                        *hash = rlp_node.as_hash();
                        (rlp_node, SparseNodeType::Leaf)
                    }
                }
                SparseNode::Extension { key, hash, store_in_db_trie } => {
                    let mut child_path = path.clone();
                    child_path.extend_from_slice_unchecked(key);
                    if let Some((hash, store_in_db_trie)) =
                        hash.zip(*store_in_db_trie).filter(|_| !prefix_set_contains(&path))
                    {
                        (
                            RlpNode::word_rlp(&hash),
                            SparseNodeType::Extension { store_in_db_trie: Some(store_in_db_trie) },
                        )
                    } else if self
                        .buffers
                        .rlp_node_stack
                        .last()
                        .is_some_and(|e| e.path == child_path)
                    {
                        let RlpNodeStackItem {
                            path: _,
                            rlp_node: child,
                            node_type: child_node_type,
                        } = self.buffers.rlp_node_stack.pop().unwrap();
                        self.buffers.rlp_buf.clear();
                        let rlp_node =
                            ExtensionNodeRef::new(key, &child).rlp(&mut self.buffers.rlp_buf);
                        *hash = rlp_node.as_hash();

                        let store_in_db_trie_value = child_node_type.store_in_db_trie();

                        trace!(
                            target: "trie::parallel_sparse",
                            ?path,
                            ?child_path,
                            ?child_node_type,
                            "Extension node"
                        );

                        *store_in_db_trie = store_in_db_trie_value;

                        (
                            rlp_node,
                            SparseNodeType::Extension {
                                // Inherit the `store_in_db_trie` flag from the child node, which is
                                // always the branch node
                                store_in_db_trie: store_in_db_trie_value,
                            },
                        )
                    } else {
                        // need to get rlp node for child first
                        self.buffers.path_stack.extend([
                            RlpNodePathStackItem { level, path, is_in_prefix_set },
                            RlpNodePathStackItem {
                                level: level + 1,
                                path: child_path,
                                is_in_prefix_set: None,
                            },
                        ]);
                        continue
                    }
                }
                SparseNode::Branch { state_mask, hash, store_in_db_trie } => {
                    if let Some((hash, store_in_db_trie)) =
                        hash.zip(*store_in_db_trie).filter(|_| !prefix_set_contains(&path))
                    {
                        self.buffers.rlp_node_stack.push(RlpNodeStackItem {
                            path,
                            rlp_node: RlpNode::word_rlp(&hash),
                            node_type: SparseNodeType::Branch {
                                store_in_db_trie: Some(store_in_db_trie),
                            },
                        });
                        continue
                    }
                    let retain_updates = updates.is_some() && prefix_set_contains(&path);

                    self.buffers.branch_child_buf.clear();
                    // Walk children in a reverse order from `f` to `0`, so we pop the `0` first
                    // from the stack and keep walking in the sorted order.
                    for bit in CHILD_INDEX_RANGE.rev() {
                        if state_mask.is_bit_set(bit) {
                            let mut child = path.clone();
                            child.push_unchecked(bit);
                            self.buffers.branch_child_buf.push(child);
                        }
                    }

                    self.buffers
                        .branch_value_stack_buf
                        .resize(self.buffers.branch_child_buf.len(), Default::default());
                    let mut added_children = false;

                    let mut tree_mask = TrieMask::default();
                    let mut hash_mask = TrieMask::default();
                    let mut hashes = Vec::new();
                    for (i, child_path) in self.buffers.branch_child_buf.iter().enumerate() {
                        if self.buffers.rlp_node_stack.last().is_some_and(|e| &e.path == child_path)
                        {
                            let RlpNodeStackItem {
                                path: _,
                                rlp_node: child,
                                node_type: child_node_type,
                            } = self.buffers.rlp_node_stack.pop().unwrap();

                            // Update the masks only if we need to retain trie updates
                            if retain_updates {
                                // SAFETY: it's a child, so it's never empty
                                let last_child_nibble = child_path.last().unwrap();

                                // Determine whether we need to set trie mask bit.
                                let should_set_tree_mask_bit = if let Some(store_in_db_trie) =
                                    child_node_type.store_in_db_trie()
                                {
                                    // A branch or an extension node explicitly set the
                                    // `store_in_db_trie` flag
                                    store_in_db_trie
                                } else {
                                    // A blinded node has the tree mask bit set
                                    child_node_type.is_hash() &&
                                        self.branch_node_tree_masks.get(&path).is_some_and(
                                            |mask| mask.is_bit_set(last_child_nibble),
                                        )
                                };
                                if should_set_tree_mask_bit {
                                    tree_mask.set_bit(last_child_nibble);
                                }

                                // Set the hash mask. If a child node is a revealed branch node OR
                                // is a blinded node that has its hash mask bit set according to the
                                // database, set the hash mask bit and save the hash.
                                let hash = child.as_hash().filter(|_| {
                                    child_node_type.is_branch() ||
                                        (child_node_type.is_hash() &&
                                            self.branch_node_hash_masks
                                                .get(&path)
                                                .is_some_and(|mask| {
                                                    mask.is_bit_set(last_child_nibble)
                                                }))
                                });
                                if let Some(hash) = hash {
                                    hash_mask.set_bit(last_child_nibble);
                                    hashes.push(hash);
                                }
                            }

                            // Insert children in the resulting buffer in a normal order,
                            // because initially we iterated in reverse.
                            // SAFETY: i < len and len is never 0
                            let original_idx = self.buffers.branch_child_buf.len() - i - 1;
                            self.buffers.branch_value_stack_buf[original_idx] = child;
                            added_children = true;
                        } else {
                            debug_assert!(!added_children);
                            self.buffers.path_stack.push(RlpNodePathStackItem {
                                level,
                                path,
                                is_in_prefix_set,
                            });
                            self.buffers.path_stack.extend(
                                self.buffers.branch_child_buf.drain(..).map(|path| {
                                    RlpNodePathStackItem {
                                        level: level + 1,
                                        path,
                                        is_in_prefix_set: None,
                                    }
                                }),
                            );
                            continue 'main
                        }
                    }

                    trace!(
                        target: "trie::parallel_sparse",
                        ?path,
                        ?tree_mask,
                        ?hash_mask,
                        "Branch node masks"
                    );

                    self.buffers.rlp_buf.clear();
                    let branch_node_ref =
                        BranchNodeRef::new(&self.buffers.branch_value_stack_buf, *state_mask);
                    let rlp_node = branch_node_ref.rlp(&mut self.buffers.rlp_buf);
                    *hash = rlp_node.as_hash();

                    // Save a branch node update only if it's not a root node, and we need to
                    // persist updates.
                    let store_in_db_trie_value = if let Some(updates) =
                        updates.as_mut().filter(|_| retain_updates && !path.is_empty())
                    {
                        let store_in_db_trie = !tree_mask.is_empty() || !hash_mask.is_empty();
                        if store_in_db_trie {
                            // Store in DB trie if there are either any children that are stored in
                            // the DB trie, or any children represent hashed values
                            hashes.reverse();
                            let branch_node = BranchNodeCompact::new(
                                *state_mask,
                                tree_mask,
                                hash_mask,
                                hashes,
                                hash.filter(|_| path.is_empty()),
                            );
                            updates.updated_nodes.insert(path.clone(), branch_node);
                        } else if self
                            .branch_node_tree_masks
                            .get(&path)
                            .is_some_and(|mask| !mask.is_empty()) ||
                            self.branch_node_hash_masks
                                .get(&path)
                                .is_some_and(|mask| !mask.is_empty())
                        {
                            // If new tree and hash masks are empty, but previously they weren't, we
                            // need to remove the node update and add the node itself to the list of
                            // removed nodes.
                            updates.updated_nodes.remove(&path);
                            updates.removed_nodes.insert(path.clone());
                        } else if self
                            .branch_node_hash_masks
                            .get(&path)
                            .is_none_or(|mask| mask.is_empty()) &&
                            self.branch_node_hash_masks
                                .get(&path)
                                .is_none_or(|mask| mask.is_empty())
                        {
                            // If new tree and hash masks are empty, and they were previously empty
                            // as well, we need to remove the node update.
                            updates.updated_nodes.remove(&path);
                        }

                        store_in_db_trie
                    } else {
                        false
                    };
                    *store_in_db_trie = Some(store_in_db_trie_value);

                    (
                        rlp_node,
                        SparseNodeType::Branch { store_in_db_trie: Some(store_in_db_trie_value) },
                    )
                }
            };

            trace!(
                target: "trie::parallel_sparse",
                root = ?self.path,
                ?level,
                ?path,
                ?node,
                ?node_type,
                ?is_in_prefix_set,
                "Added node to rlp node stack"
            );

            self.buffers.rlp_node_stack.push(RlpNodeStackItem { path, rlp_node, node_type });
        }

        debug_assert_eq!(self.buffers.rlp_node_stack.len(), 1);
        (self.buffers.rlp_node_stack.pop().unwrap().rlp_node, updates)
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

/// Collection of reusable buffers for [`RevealedSparseTrie::rlp_node`] calculations.
///
/// These buffers reduce allocations when computing RLP representations during trie updates.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SparseSubtrieBuffers {
    /// Stack of RLP node paths
    path_stack: Vec<RlpNodePathStackItem>,
    /// Stack of RLP nodes
    rlp_node_stack: Vec<RlpNodeStackItem>,
    /// Reusable branch child path
    branch_child_buf: SmallVec<[Nibbles; 16]>,
    /// Reusable branch value stack
    branch_value_stack_buf: SmallVec<[RlpNode; 16]>,
    /// Reusable RLP buffer
    rlp_buf: Vec<u8>,
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
        trie.lower_subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.lower_subtries[subtrie_2_index] = Some(subtrie_2.clone());
        trie.lower_subtries[subtrie_3_index] = Some(subtrie_3);

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
                    (subtrie, prefix_set.iter().cloned().collect::<Vec<_>>())
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
        assert!(trie.lower_subtries[subtrie_2_index].is_none());

        // First subtrie should remain unchanged
        assert_eq!(trie.lower_subtries[subtrie_1_index], Some(subtrie_1));
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
        trie.lower_subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.lower_subtries[subtrie_2_index] = Some(subtrie_2.clone());
        trie.lower_subtries[subtrie_3_index] = Some(subtrie_3.clone());

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
        assert!(trie.lower_subtries.iter().all(Option::is_none));
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
