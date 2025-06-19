use std::sync::mpsc;

use alloy_primitives::{
    map::{Entry, HashMap},
    B256,
};
use alloy_rlp::Decodable;
use alloy_trie::{BranchNodeCompact, TrieMask, EMPTY_ROOT_HASH};
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{
    prefix_set::{PrefixSet, PrefixSetMut},
    BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, RlpNode, TrieNode, CHILD_INDEX_RANGE,
};
use reth_trie_sparse::{
    blinded::BlindedProvider, RlpNodePathStackItem, RlpNodeStackItem, SparseNode, SparseNodeType,
    SparseTrieUpdates, TrieMasks,
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
    upper_subtrie: Box<SparseSubtrie>,
    /// An array containing the subtries at the second level of the trie.
    lower_subtries: [Option<Box<SparseSubtrie>>; 256],
    /// Set of prefixes (key paths) that have been marked as updated.
    /// This is used to track which parts of the trie need to be recalculated.
    prefix_set: PrefixSetMut,
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            upper_subtrie: Box::default(),
            lower_subtries: [const { None }; 256],
            prefix_set: PrefixSetMut::default(),
            updates: None,
        }
    }
}

impl ParallelSparseTrie {
    /// Returns mutable ref to the lower `SparseSubtrie` for the given path, or None if the path
    /// belongs to the upper trie.
    fn lower_subtrie_for_path(&mut self, path: &Nibbles) -> Option<&mut Box<SparseSubtrie>> {
        match SparseSubtrieType::from_path(path) {
            SparseSubtrieType::Upper => None,
            SparseSubtrieType::Lower(idx) => {
                if self.lower_subtries[idx].is_none() {
                    let upper_path = path.slice(..2);
                    self.lower_subtries[idx] = Some(Box::new(SparseSubtrie::new(upper_path)));
                }

                self.lower_subtries[idx].as_mut()
            }
        }
    }

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
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if node was not revealed.
    pub fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNode,
        masks: TrieMasks,
    ) -> SparseTrieResult<()> {
        // TODO parallelize
        if let Some(subtrie) = self.lower_subtrie_for_path(&path) {
            return subtrie.reveal_node(path, &node, masks);
        }

        // If there is no subtrie for the path it means the path is 2 or less nibbles, and so
        // belongs to the upper trie.
        self.upper_subtrie.reveal_node(path.clone(), &node, masks)?;

        // The previous upper_trie.reveal_node call will not have revealed any child nodes via
        // reveal_node_or_hash if the child node would be found on a lower subtrie. We handle that
        // here by manually checking the specific cases where this could happen, and calling
        // reveal_node_or_hash for each.
        match node {
            TrieNode::Branch(branch) => {
                // If a branch is at the second level of the trie then it will be in the upper trie,
                // but all of its children will be in the lower trie.
                if path.len() == 2 {
                    let mut stack_ptr = branch.as_ref().first_child_index();
                    for idx in CHILD_INDEX_RANGE {
                        if branch.state_mask.is_bit_set(idx) {
                            let mut child_path = path.clone();
                            child_path.push_unchecked(idx);
                            self.lower_subtrie_for_path(&child_path)
                                .expect("child_path must have a lower subtrie")
                                .reveal_node_or_hash(child_path, &branch.stack[stack_ptr])?;
                            stack_ptr += 1;
                        }
                    }
                }
            }
            TrieNode::Extension(ext) => {
                let mut child_path = path.clone();
                child_path.extend_from_slice_unchecked(&ext.key);
                if child_path.len() > 2 {
                    self.lower_subtrie_for_path(&child_path)
                        .expect("child_path must have a lower subtrie")
                        .reveal_node_or_hash(child_path, &ext.child)?;
                }
            }
            TrieNode::EmptyRoot | TrieNode::Leaf(_) => (),
        }

        Ok(())
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
    pub fn update_subtrie_hashes(&mut self) {
        trace!(target: "trie::parallel_sparse", "Updating subtrie hashes");

        // Take changed subtries according to the prefix set
        let mut prefix_set = core::mem::take(&mut self.prefix_set).freeze();
        let (subtries, unchanged_prefix_set) = self.take_changed_lower_subtries(&mut prefix_set);

        // Update the prefix set with the keys that didn't have matching subtries
        self.prefix_set = unchanged_prefix_set;

        // Update subtrie hashes in parallel
        // TODO: call `update_hashes` on each subtrie in parallel
        let (tx, rx) = mpsc::channel();
        for subtrie in subtries {
            tx.send((subtrie.index, subtrie.subtrie)).unwrap();
        }
        drop(tx);

        // Return updated subtries back to the trie
        for (index, subtrie) in rx {
            self.lower_subtries[index] = Some(subtrie);
        }
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
        self.updates = retain_updates.then_some(SparseTrieUpdates::default());
        self
    }

    /// Consumes and returns the currently accumulated trie updates.
    ///
    /// This is useful when you want to apply the updates to an external database,
    /// and then start tracking a new set of updates.
    pub fn take_updates(&mut self) -> SparseTrieUpdates {
        core::iter::once(&mut self.upper_subtrie)
            .chain(self.lower_subtries.iter_mut().flatten())
            .fold(SparseTrieUpdates::default(), |mut acc, subtrie| {
                acc.extend(subtrie.take_updates());
                acc
            })
    }

    /// Returns:
    /// 1. List of lower [subtries](SparseSubtrie) that have changed according to the provided
    ///    [prefix set](PrefixSet). See documentation of [`ChangedSubtrie`] for more details.
    /// 2. Prefix set of keys that do not belong to any lower subtrie.
    ///
    /// This method helps optimize hash recalculations by identifying which specific
    /// lower subtries need to be updated. Each lower subtrie can then be updated in parallel.
    ///
    /// IMPORTANT: The method removes the subtries from `lower_subtries`, and the caller is
    /// responsible for returning them back into the array.
    fn take_changed_lower_subtries(
        &mut self,
        prefix_set: &mut PrefixSet,
    ) -> (Vec<ChangedSubtrie>, PrefixSetMut) {
        // Clone the prefix set to iterate over its keys. Cloning is cheap, it's just an Arc.
        let prefix_set_clone = prefix_set.clone();
        let mut prefix_set_iter = prefix_set_clone.into_iter().cloned().peekable();
        let mut changed_subtries = Vec::new();
        let mut unchanged_prefix_set = PrefixSetMut::default();

        for (index, subtrie) in self.lower_subtries.iter_mut().enumerate() {
            if let Some(subtrie) = subtrie.take_if(|subtrie| prefix_set.contains(&subtrie.path)) {
                let prefix_set = if prefix_set.all() {
                    unchanged_prefix_set = PrefixSetMut::all();
                    PrefixSetMut::all()
                } else {
                    // Take those keys from the original prefix set that start with the subtrie path
                    //
                    // Subtries are stored in the order of their paths, so we can use the same
                    // prefix set iterator.
                    let mut new_prefix_set = Vec::new();
                    while let Some(key) = prefix_set_iter.peek() {
                        if key.has_prefix(&subtrie.path) {
                            // If the key starts with the subtrie path, add it to the new prefix set
                            new_prefix_set.push(prefix_set_iter.next().unwrap());
                        } else if new_prefix_set.is_empty() && key < &subtrie.path {
                            // If we didn't yet have any keys that belong to this subtrie, and the
                            // current key is still less than the subtrie path, add it to the
                            // unchanged prefix set
                            unchanged_prefix_set.insert(prefix_set_iter.next().unwrap());
                        } else {
                            // If we're past the subtrie path, we're done with this subtrie. Do not
                            // advance the iterator, the next key will be processed either by the
                            // next subtrie or inserted into the unchanged prefix set.
                            break
                        }
                    }
                    PrefixSetMut::from(new_prefix_set)
                }
                .freeze();

                changed_subtries.push(ChangedSubtrie { index, subtrie, prefix_set });
            }
        }

        // Extend the unchanged prefix set with the remaining keys that are not part of any subtries
        unchanged_prefix_set.extend_keys(prefix_set_iter);

        (changed_subtries, unchanged_prefix_set)
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
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
    /// Reusable buffers for [`SparseSubtrie::update_hashes`].
    buffers: SparseSubtrieBuffers,
}

impl SparseSubtrie {
    fn new(path: Nibbles) -> Self {
        Self { path, ..Default::default() }
    }

    /// Configures the subtrie to retain information about updates.
    ///
    /// If `retain_updates` is true, the trie will record branch node updates and deletions.
    /// This information can then be used to efficiently update an external database.
    pub fn with_updates(mut self, retain_updates: bool) -> Self {
        self.updates = retain_updates.then_some(SparseTrieUpdates::default());
        self
    }

    /// Returns true if the current path and its child are both found in the same level. This
    /// function assumes that if `current_path` is in a lower level then `child_path` is too.
    fn is_child_same_level(current_path: &Nibbles, child_path: &Nibbles) -> bool {
        let current_level = core::mem::discriminant(&SparseSubtrieType::from_path(current_path));
        let child_level = core::mem::discriminant(&SparseSubtrieType::from_path(child_path));
        current_level == child_level
    }

    /// Internal implementation of the method of the same name on `ParallelSparseTrie`.
    fn reveal_node(
        &mut self,
        path: Nibbles,
        node: &TrieNode,
        masks: TrieMasks,
    ) -> SparseTrieResult<()> {
        // If the node is already revealed and it's not a hash node, do nothing.
        if self.nodes.get(&path).is_some_and(|node| !node.is_hash()) {
            return Ok(())
        }

        if let Some(tree_mask) = masks.tree_mask {
            self.branch_node_tree_masks.insert(path.clone(), tree_mask);
        }
        if let Some(hash_mask) = masks.hash_mask {
            self.branch_node_hash_masks.insert(path.clone(), hash_mask);
        }

        match node {
            TrieNode::EmptyRoot => {
                // For an empty root, ensure that we are at the root path, and at the upper subtrie.
                debug_assert!(path.is_empty());
                debug_assert!(self.path.is_empty());
                self.nodes.insert(path, SparseNode::Empty);
            }
            TrieNode::Branch(branch) => {
                // For a branch node, iterate over all potential children
                let mut stack_ptr = branch.as_ref().first_child_index();
                for idx in CHILD_INDEX_RANGE {
                    if branch.state_mask.is_bit_set(idx) {
                        let mut child_path = path.clone();
                        child_path.push_unchecked(idx);
                        if Self::is_child_same_level(&path, &child_path) {
                            // Reveal each child node or hash it has, but only if the child is on
                            // the same level as the parent.
                            self.reveal_node_or_hash(child_path, &branch.stack[stack_ptr])?;
                        }
                        stack_ptr += 1;
                    }
                }
                // Update the branch node entry in the nodes map, handling cases where a blinded
                // node is now replaced with a revealed node.
                match self.nodes.entry(path) {
                    Entry::Occupied(mut entry) => match entry.get() {
                        // Replace a hash node with a fully revealed branch node.
                        SparseNode::Hash(hash) => {
                            entry.insert(SparseNode::Branch {
                                state_mask: branch.state_mask,
                                // Memoize the hash of a previously blinded node in a new branch
                                // node.
                                hash: Some(*hash),
                                store_in_db_trie: Some(
                                    masks.hash_mask.is_some_and(|mask| !mask.is_empty()) ||
                                        masks.tree_mask.is_some_and(|mask| !mask.is_empty()),
                                ),
                            });
                        }
                        // Branch node already exists, or an extension node was placed where a
                        // branch node was before.
                        SparseNode::Branch { .. } | SparseNode::Extension { .. } => {}
                        // All other node types can't be handled.
                        node @ (SparseNode::Empty | SparseNode::Leaf { .. }) => {
                            return Err(SparseTrieErrorKind::Reveal {
                                path: entry.key().clone(),
                                node: Box::new(node.clone()),
                            }
                            .into())
                        }
                    },
                    Entry::Vacant(entry) => {
                        entry.insert(SparseNode::new_branch(branch.state_mask));
                    }
                }
            }
            TrieNode::Extension(ext) => match self.nodes.entry(path.clone()) {
                Entry::Occupied(mut entry) => match entry.get() {
                    // Replace a hash node with a revealed extension node.
                    SparseNode::Hash(hash) => {
                        let mut child_path = entry.key().clone();
                        child_path.extend_from_slice_unchecked(&ext.key);
                        entry.insert(SparseNode::Extension {
                            key: ext.key.clone(),
                            // Memoize the hash of a previously blinded node in a new extension
                            // node.
                            hash: Some(*hash),
                            store_in_db_trie: None,
                        });
                        if Self::is_child_same_level(&path, &child_path) {
                            self.reveal_node_or_hash(child_path, &ext.child)?;
                        }
                    }
                    // Extension node already exists, or an extension node was placed where a branch
                    // node was before.
                    SparseNode::Extension { .. } | SparseNode::Branch { .. } => {}
                    // All other node types can't be handled.
                    node @ (SparseNode::Empty | SparseNode::Leaf { .. }) => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: entry.key().clone(),
                            node: Box::new(node.clone()),
                        }
                        .into())
                    }
                },
                Entry::Vacant(entry) => {
                    let mut child_path = entry.key().clone();
                    child_path.extend_from_slice_unchecked(&ext.key);
                    entry.insert(SparseNode::new_ext(ext.key.clone()));
                    if Self::is_child_same_level(&path, &child_path) {
                        self.reveal_node_or_hash(child_path, &ext.child)?;
                    }
                }
            },
            TrieNode::Leaf(leaf) => match self.nodes.entry(path) {
                Entry::Occupied(mut entry) => match entry.get() {
                    // Replace a hash node with a revealed leaf node and store leaf node value.
                    SparseNode::Hash(hash) => {
                        let mut full = entry.key().clone();
                        full.extend_from_slice_unchecked(&leaf.key);
                        self.values.insert(full, leaf.value.clone());
                        entry.insert(SparseNode::Leaf {
                            key: leaf.key.clone(),
                            // Memoize the hash of a previously blinded node in a new leaf
                            // node.
                            hash: Some(*hash),
                        });
                    }
                    // Leaf node already exists.
                    SparseNode::Leaf { .. } => {}
                    // All other node types can't be handled.
                    node @ (SparseNode::Empty |
                    SparseNode::Extension { .. } |
                    SparseNode::Branch { .. }) => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: entry.key().clone(),
                            node: Box::new(node.clone()),
                        }
                        .into())
                    }
                },
                Entry::Vacant(entry) => {
                    let mut full = entry.key().clone();
                    full.extend_from_slice_unchecked(&leaf.key);
                    entry.insert(SparseNode::new_leaf(leaf.key.clone()));
                    self.values.insert(full, leaf.value.clone());
                }
            },
        }

        Ok(())
    }

    /// Reveals either a node or its hash placeholder based on the provided child data.
    ///
    /// When traversing the trie, we often encounter references to child nodes that
    /// are either directly embedded or represented by their hash. This method
    /// handles both cases:
    ///
    /// 1. If the child data represents a hash (32+1=33 bytes), store it as a hash node
    /// 2. Otherwise, decode the data as a [`TrieNode`] and recursively reveal it using
    ///    `reveal_node`
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if successful, or an error if the node cannot be revealed.
    ///
    /// # Error Handling
    ///
    /// Will error if there's a conflict between a new hash node and an existing one
    /// at the same path
    fn reveal_node_or_hash(&mut self, path: Nibbles, child: &[u8]) -> SparseTrieResult<()> {
        if child.len() == B256::len_bytes() + 1 {
            let hash = B256::from_slice(&child[1..]);
            match self.nodes.entry(path) {
                Entry::Occupied(entry) => match entry.get() {
                    // Hash node with a different hash can't be handled.
                    SparseNode::Hash(previous_hash) if previous_hash != &hash => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: entry.key().clone(),
                            node: Box::new(SparseNode::Hash(hash)),
                        }
                        .into())
                    }
                    _ => {}
                },
                Entry::Vacant(entry) => {
                    entry.insert(SparseNode::Hash(hash));
                }
            }
            return Ok(())
        }

        self.reveal_node(path, &TrieNode::decode(&mut &child[..])?, TrieMasks::none())
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
    #[allow(unused)]
    pub fn update_hashes(&mut self, prefix_set: &mut PrefixSet) -> RlpNode {
        trace!(target: "trie::parallel_sparse", root=?self.path, "Updating subtrie hashes");

        debug_assert!(self.buffers.path_stack.is_empty());
        self.buffers.path_stack.push(RlpNodePathStackItem {
            level: 0,
            path: self.path.clone(),
            is_in_prefix_set: None,
        });

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
                    let retain_updates = self.updates.is_some() && prefix_set_contains(&path);

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
                        self.updates.as_mut().filter(|_| retain_updates && !path.is_empty())
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
        self.buffers.rlp_node_stack.pop().unwrap().rlp_node
    }

    /// Consumes and returns the currently accumulated trie updates.
    ///
    /// This is useful when you want to apply the updates to an external database,
    /// and then start tracking a new set of updates.
    fn take_updates(&mut self) -> SparseTrieUpdates {
        self.updates.take().unwrap_or_default()
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

    /// Returns the index of the lower subtrie, if it exists.
    pub const fn lower_index(&self) -> Option<usize> {
        match self {
            Self::Upper => None,
            Self::Lower(index) => Some(*index),
        }
    }
}

/// Collection of reusable buffers for calculating subtrie hashes.
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

/// Changed subtrie.
#[derive(Debug)]
struct ChangedSubtrie {
    /// Lower subtrie index in the range [0, 255].
    index: usize,
    /// Changed subtrie
    subtrie: Box<SparseSubtrie>,
    /// Prefix set of keys that belong to the subtrie.
    #[allow(unused)]
    prefix_set: PrefixSet,
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
    use super::{
        path_subtrie_index_unchecked, ParallelSparseTrie, SparseSubtrie, SparseSubtrieType,
    };
    use crate::trie::ChangedSubtrie;
    use alloy_primitives::B256;
    use alloy_rlp::Encodable;
    use alloy_trie::Nibbles;
    use assert_matches::assert_matches;
    use reth_primitives_traits::Account;
    use reth_trie_common::{
        prefix_set::PrefixSetMut, BranchNode, ExtensionNode, LeafNode, RlpNode, TrieMask, TrieNode,
        EMPTY_ROOT_HASH,
    };
    use reth_trie_sparse::{SparseNode, TrieMasks};

    // Test helpers
    fn encode_account_value(nonce: u64) -> Vec<u8> {
        let account = Account { nonce, ..Default::default() };
        let trie_account = account.into_trie_account(EMPTY_ROOT_HASH);
        let mut buf = Vec::new();
        trie_account.encode(&mut buf);
        buf
    }

    fn create_leaf_node(key: &[u8], value_nonce: u64) -> TrieNode {
        TrieNode::Leaf(LeafNode::new(
            Nibbles::from_nibbles_unchecked(key),
            encode_account_value(value_nonce),
        ))
    }

    fn create_extension_node(key: &[u8], child_hash: B256) -> TrieNode {
        TrieNode::Extension(ExtensionNode::new(
            Nibbles::from_nibbles_unchecked(key),
            RlpNode::word_rlp(&child_hash),
        ))
    }

    fn create_branch_node_with_children(
        children_indices: &[u8],
        child_hashes: &[B256],
    ) -> TrieNode {
        let mut stack = Vec::new();
        let mut state_mask = 0u16;

        for (&idx, &hash) in children_indices.iter().zip(child_hashes.iter()) {
            state_mask |= 1 << idx;
            stack.push(RlpNode::word_rlp(&hash));
        }

        TrieNode::Branch(BranchNode::new(stack, TrieMask::new(state_mask)))
    }

    #[test]
    fn test_get_changed_subtries_empty() {
        let mut trie = ParallelSparseTrie::default();
        let mut prefix_set = PrefixSetMut::from([Nibbles::default()]).freeze();

        let (subtries, unchanged_prefix_set) = trie.take_changed_lower_subtries(&mut prefix_set);
        assert!(subtries.is_empty());
        assert_eq!(unchanged_prefix_set, PrefixSetMut::from(prefix_set.iter().cloned()));
    }

    #[test]
    fn test_get_changed_subtries() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0])));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0])));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0])));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.lower_subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.lower_subtries[subtrie_2_index] = Some(subtrie_2.clone());
        trie.lower_subtries[subtrie_3_index] = Some(subtrie_3);

        let unchanged_prefix_set = PrefixSetMut::from([
            Nibbles::from_nibbles_unchecked([0x0]),
            Nibbles::from_nibbles_unchecked([0x2, 0x0, 0x0]),
        ]);
        // Create a prefix set with the keys that match only the second subtrie
        let mut prefix_set = PrefixSetMut::from([
            // Match second subtrie
            Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x0]),
            Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x1, 0x0]),
        ]);
        prefix_set.extend(unchanged_prefix_set);
        let mut prefix_set = prefix_set.freeze();

        // Second subtrie should be removed and returned
        let (subtries, unchanged_prefix_set) = trie.take_changed_lower_subtries(&mut prefix_set);
        assert_eq!(
            subtries
                .into_iter()
                .map(|ChangedSubtrie { index, subtrie, prefix_set }| {
                    (index, subtrie, prefix_set.iter().cloned().collect::<Vec<_>>())
                })
                .collect::<Vec<_>>(),
            vec![(
                subtrie_2_index,
                subtrie_2,
                vec![
                    Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x0]),
                    Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x1, 0x0])
                ]
            )]
        );
        assert_eq!(unchanged_prefix_set, unchanged_prefix_set);
        assert!(trie.lower_subtries[subtrie_2_index].is_none());

        // First subtrie should remain unchanged
        assert_eq!(trie.lower_subtries[subtrie_1_index], Some(subtrie_1));
    }

    #[test]
    fn test_get_changed_subtries_all() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0])));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0])));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0])));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.lower_subtries[subtrie_1_index] = Some(subtrie_1.clone());
        trie.lower_subtries[subtrie_2_index] = Some(subtrie_2.clone());
        trie.lower_subtries[subtrie_3_index] = Some(subtrie_3.clone());

        // Create a prefix set that matches any key
        let mut prefix_set = PrefixSetMut::all().freeze();

        // All subtries should be removed and returned
        let (subtries, unchanged_prefix_set) = trie.take_changed_lower_subtries(&mut prefix_set);
        assert_eq!(
            subtries
                .into_iter()
                .map(|ChangedSubtrie { index, subtrie, prefix_set }| {
                    (index, subtrie, prefix_set.all())
                })
                .collect::<Vec<_>>(),
            vec![
                (subtrie_1_index, subtrie_1, true),
                (subtrie_2_index, subtrie_2, true),
                (subtrie_3_index, subtrie_3, true)
            ]
        );
        assert_eq!(unchanged_prefix_set, PrefixSetMut::all());

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

    #[test]
    fn reveal_node_leaves() {
        let mut trie = ParallelSparseTrie::default();

        // Reveal leaf in the upper trie
        {
            let path = Nibbles::from_nibbles([0x1, 0x2]);
            let node = create_leaf_node(&[0x3, 0x4], 42);
            let masks = TrieMasks::none();

            trie.reveal_node(path.clone(), node, masks).unwrap();

            assert_matches!(
                trie.upper_subtrie.nodes.get(&path),
                Some(SparseNode::Leaf { key, hash: None })
                if key == &Nibbles::from_nibbles([0x3, 0x4])
            );

            let full_path = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
            assert_eq!(trie.upper_subtrie.values.get(&full_path), Some(&encode_account_value(42)));
        }

        // Reveal leaf in a lower trie
        {
            let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
            let node = create_leaf_node(&[0x4, 0x5], 42);
            let masks = TrieMasks::none();

            trie.reveal_node(path.clone(), node, masks).unwrap();

            // Check that the lower subtrie was created
            let idx = path_subtrie_index_unchecked(&path);
            assert!(trie.lower_subtries[idx].is_some());

            let lower_subtrie = trie.lower_subtries[idx].as_ref().unwrap();
            assert_matches!(
                lower_subtrie.nodes.get(&path),
                Some(SparseNode::Leaf { key, hash: None })
                if key == &Nibbles::from_nibbles([0x4, 0x5])
            );
        }
    }

    #[test]
    fn reveal_node_extension_all_upper() {
        let mut trie = ParallelSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1]);
        let child_hash = B256::repeat_byte(0xab);
        let node = create_extension_node(&[0x2], child_hash);
        let masks = TrieMasks::none();

        trie.reveal_node(path.clone(), node, masks).unwrap();

        assert_matches!(
            trie.upper_subtrie.nodes.get(&path),
            Some(SparseNode::Extension { key, hash: None, .. })
            if key == &Nibbles::from_nibbles([0x2])
        );

        // Child path should be in upper trie
        let child_path = Nibbles::from_nibbles([0x1, 0x2]);
        assert_eq!(trie.upper_subtrie.nodes.get(&child_path), Some(&SparseNode::Hash(child_hash)));
    }

    #[test]
    fn reveal_node_extension_cross_level() {
        let mut trie = ParallelSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1, 0x2]);
        let child_hash = B256::repeat_byte(0xcd);
        let node = create_extension_node(&[0x3], child_hash);
        let masks = TrieMasks::none();

        trie.reveal_node(path.clone(), node, masks).unwrap();

        // Extension node should be in upper trie
        assert_matches!(
            trie.upper_subtrie.nodes.get(&path),
            Some(SparseNode::Extension { key, hash: None, .. })
            if key == &Nibbles::from_nibbles([0x3])
        );

        // Child path (0x1, 0x2, 0x3) should be in lower trie
        let child_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let idx = path_subtrie_index_unchecked(&child_path);
        assert!(trie.lower_subtries[idx].is_some());

        let lower_subtrie = trie.lower_subtries[idx].as_ref().unwrap();
        assert_eq!(lower_subtrie.nodes.get(&child_path), Some(&SparseNode::Hash(child_hash)));
    }

    #[test]
    fn reveal_node_branch_all_upper() {
        let mut trie = ParallelSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1]);
        let child_hashes = [B256::repeat_byte(0x11), B256::repeat_byte(0x22)];
        let node = create_branch_node_with_children(&[0x0, 0x5], &child_hashes);
        let masks = TrieMasks::none();

        trie.reveal_node(path.clone(), node, masks).unwrap();

        // Branch node should be in upper trie
        assert_matches!(
            trie.upper_subtrie.nodes.get(&path),
            Some(SparseNode::Branch { state_mask, hash: None, .. })
            if *state_mask == 0b0000000000100001.into()
        );

        // Children should be in upper trie (paths of length 2)
        let child_path_0 = Nibbles::from_nibbles([0x1, 0x0]);
        let child_path_5 = Nibbles::from_nibbles([0x1, 0x5]);
        assert_eq!(
            trie.upper_subtrie.nodes.get(&child_path_0),
            Some(&SparseNode::Hash(child_hashes[0]))
        );
        assert_eq!(
            trie.upper_subtrie.nodes.get(&child_path_5),
            Some(&SparseNode::Hash(child_hashes[1]))
        );
    }

    #[test]
    fn reveal_node_branch_cross_level() {
        let mut trie = ParallelSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1, 0x2]); // Exactly 2 nibbles - boundary case
        let child_hashes =
            [B256::repeat_byte(0x33), B256::repeat_byte(0x44), B256::repeat_byte(0x55)];
        let node = create_branch_node_with_children(&[0x0, 0x7, 0xf], &child_hashes);
        let masks = TrieMasks::none();

        trie.reveal_node(path.clone(), node, masks).unwrap();

        // Branch node should be in upper trie
        assert_matches!(
            trie.upper_subtrie.nodes.get(&path),
            Some(SparseNode::Branch { state_mask, hash: None, .. })
            if *state_mask == 0b1000000010000001.into()
        );

        // All children should be in lower tries since they have paths of length 3
        let child_paths = [
            Nibbles::from_nibbles([0x1, 0x2, 0x0]),
            Nibbles::from_nibbles([0x1, 0x2, 0x7]),
            Nibbles::from_nibbles([0x1, 0x2, 0xf]),
        ];

        for (i, child_path) in child_paths.iter().enumerate() {
            let idx = path_subtrie_index_unchecked(child_path);
            let lower_subtrie = trie.lower_subtries[idx].as_ref().unwrap();
            assert_eq!(
                lower_subtrie.nodes.get(child_path),
                Some(&SparseNode::Hash(child_hashes[i])),
            );
        }
    }

    #[test]
    fn test_update_subtrie_hashes() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0])));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0])));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0])));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.lower_subtries[subtrie_1_index] = Some(subtrie_1);
        trie.lower_subtries[subtrie_2_index] = Some(subtrie_2);
        trie.lower_subtries[subtrie_3_index] = Some(subtrie_3);

        let unchanged_prefix_set = PrefixSetMut::from([
            Nibbles::from_nibbles_unchecked([0x0]),
            Nibbles::from_nibbles_unchecked([0x2, 0x0, 0x0]),
        ]);
        // Create a prefix set with the keys that match only the second subtrie
        let mut prefix_set = PrefixSetMut::from([
            // Match second subtrie
            Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x0]),
            Nibbles::from_nibbles_unchecked([0x1, 0x0, 0x1, 0x0]),
        ]);
        prefix_set.extend(unchanged_prefix_set.clone());
        trie.prefix_set = prefix_set;

        // Update subtrie hashes
        trie.update_subtrie_hashes();

        // Check that the prefix set was updated
        assert_eq!(trie.prefix_set, unchanged_prefix_set);
        // Check that subtries were returned back to the array
        assert!(trie.lower_subtries[subtrie_1_index].is_some());
        assert!(trie.lower_subtries[subtrie_2_index].is_some());
        assert!(trie.lower_subtries[subtrie_3_index].is_some());
    }
}
