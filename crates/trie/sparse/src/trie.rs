use crate::{
    blinded::{BlindedProvider, DefaultBlindedProvider},
    SparseTrieError, SparseTrieResult,
};
use alloy_primitives::{
    hex, keccak256,
    map::{HashMap, HashSet},
    B256,
};
use alloy_rlp::Decodable;
use reth_tracing::tracing::trace;
use reth_trie_common::{
    prefix_set::{PrefixSet, PrefixSetMut},
    BranchNodeCompact, BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, RlpNode, TrieMask,
    TrieNode, CHILD_INDEX_RANGE, EMPTY_ROOT_HASH,
};
use smallvec::SmallVec;
use std::{borrow::Cow, fmt};

/// Inner representation of the sparse trie.
/// Sparse trie is blind by default until nodes are revealed.
#[derive(PartialEq, Eq)]
pub enum SparseTrie<P = DefaultBlindedProvider> {
    /// None of the trie nodes are known.
    Blind,
    /// The trie nodes have been revealed.
    Revealed(Box<RevealedSparseTrie<P>>),
}

impl<P> fmt::Debug for SparseTrie<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blind => write!(f, "Blind"),
            Self::Revealed(revealed) => write!(f, "Revealed({revealed:?})"),
        }
    }
}

impl<P> Default for SparseTrie<P> {
    fn default() -> Self {
        Self::Blind
    }
}

impl SparseTrie {
    /// Creates new blind trie.
    pub const fn blind() -> Self {
        Self::Blind
    }

    /// Creates new revealed empty trie.
    pub fn revealed_empty() -> Self {
        Self::Revealed(Box::default())
    }

    /// Reveals the root node if the trie is blinded.
    ///
    /// # Returns
    ///
    /// Mutable reference to [`RevealedSparseTrie`].
    pub fn reveal_root(
        &mut self,
        root: TrieNode,
        retain_updates: bool,
    ) -> SparseTrieResult<&mut RevealedSparseTrie> {
        self.reveal_root_with_provider(Default::default(), root, retain_updates)
    }
}

impl<P> SparseTrie<P> {
    /// Returns `true` if the sparse trie has no revealed nodes.
    pub const fn is_blind(&self) -> bool {
        matches!(self, Self::Blind)
    }

    /// Returns mutable reference to revealed sparse trie if the trie is not blind.
    pub fn as_revealed_mut(&mut self) -> Option<&mut RevealedSparseTrie<P>> {
        if let Self::Revealed(revealed) = self {
            Some(revealed)
        } else {
            None
        }
    }

    /// Reveals the root node if the trie is blinded.
    ///
    /// # Returns
    ///
    /// Mutable reference to [`RevealedSparseTrie`].
    pub fn reveal_root_with_provider(
        &mut self,
        provider: P,
        root: TrieNode,
        retain_updates: bool,
    ) -> SparseTrieResult<&mut RevealedSparseTrie<P>> {
        if self.is_blind() {
            *self = Self::Revealed(Box::new(RevealedSparseTrie::from_provider_and_root(
                provider,
                root,
                retain_updates,
            )?))
        }
        Ok(self.as_revealed_mut().unwrap())
    }

    /// Update the leaf node.
    pub fn update_leaf(&mut self, path: Nibbles, value: Vec<u8>) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieError::Blind)?;
        revealed.update_leaf(path, value)?;
        Ok(())
    }

    /// Wipe the trie, removing all values and nodes, and replacing the root with an empty node.
    pub fn wipe(&mut self) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieError::Blind)?;
        revealed.wipe();
        Ok(())
    }

    /// Calculates and returns the trie root if the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        Some(self.as_revealed_mut()?.root())
    }

    /// Calculates the hashes of the nodes below the provided level.
    pub fn calculate_below_level(&mut self, level: usize) {
        self.as_revealed_mut().unwrap().update_rlp_node_level(level);
    }
}

impl<P> SparseTrie<P>
where
    P: BlindedProvider,
    SparseTrieError: From<P::Error>,
{
    /// Remove the leaf node.
    pub fn remove_leaf(&mut self, path: &Nibbles) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieError::Blind)?;
        revealed.remove_leaf(path)?;
        Ok(())
    }
}

/// The representation of revealed sparse trie.
///
/// ## Invariants
///
/// - The root node is always present in `nodes` collection.
/// - Each leaf entry in `nodes` collection must have a corresponding entry in `values` collection.
///   The opposite is also true.
/// - All keys in `values` collection are full leaf paths.
#[derive(Clone, PartialEq, Eq)]
pub struct RevealedSparseTrie<P = DefaultBlindedProvider> {
    /// Blinded node provider.
    provider: P,
    /// All trie nodes.
    nodes: HashMap<Nibbles, SparseNode>,
    /// All leaf values.
    values: HashMap<Nibbles, Vec<u8>>,
    /// Prefix set.
    prefix_set: PrefixSetMut,
    /// Retained trie updates.
    updates: Option<SparseTrieUpdates>,
    /// Reusable buffer for RLP encoding of nodes.
    rlp_buf: Vec<u8>,
}

impl<P> fmt::Debug for RevealedSparseTrie<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevealedSparseTrie")
            .field("nodes", &self.nodes)
            .field("values", &self.values)
            .field("prefix_set", &self.prefix_set)
            .field("updates", &self.updates)
            .field("rlp_buf", &hex::encode(&self.rlp_buf))
            .finish_non_exhaustive()
    }
}

impl Default for RevealedSparseTrie {
    fn default() -> Self {
        Self {
            provider: Default::default(),
            nodes: HashMap::from_iter([(Nibbles::default(), SparseNode::Empty)]),
            values: HashMap::default(),
            prefix_set: PrefixSetMut::default(),
            updates: None,
            rlp_buf: Vec::new(),
        }
    }
}

impl RevealedSparseTrie {
    /// Create new revealed sparse trie from the given root node.
    pub fn from_root(node: TrieNode, retain_updates: bool) -> SparseTrieResult<Self> {
        let mut this = Self {
            provider: Default::default(),
            nodes: HashMap::default(),
            values: HashMap::default(),
            prefix_set: PrefixSetMut::default(),
            rlp_buf: Vec::new(),
            updates: None,
        }
        .with_updates(retain_updates);
        this.reveal_node(Nibbles::default(), node)?;
        Ok(this)
    }
}

impl<P> RevealedSparseTrie<P> {
    /// Create new revealed sparse trie from the given root node.
    pub fn from_provider_and_root(
        provider: P,
        node: TrieNode,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        let mut this = Self {
            provider,
            nodes: HashMap::default(),
            values: HashMap::default(),
            prefix_set: PrefixSetMut::default(),
            rlp_buf: Vec::new(),
            updates: None,
        }
        .with_updates(retain_updates);
        this.reveal_node(Nibbles::default(), node)?;
        Ok(this)
    }

    /// Set new blinded node provider on sparse trie.
    pub fn with_provider<BP>(self, provider: BP) -> RevealedSparseTrie<BP> {
        RevealedSparseTrie {
            provider,
            nodes: self.nodes,
            values: self.values,
            prefix_set: self.prefix_set,
            updates: self.updates,
            rlp_buf: self.rlp_buf,
        }
    }

    /// Set the retention of branch node updates and deletions.
    pub fn with_updates(mut self, retain_updates: bool) -> Self {
        if retain_updates {
            self.updates = Some(SparseTrieUpdates::default());
        }
        self
    }

    /// Returns a reference to the retained sparse node updates without taking them.
    pub fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.updates.as_ref().map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    /// Returns a reference to the leaf value if present.
    pub fn get_leaf_value(&self, path: &Nibbles) -> Option<&Vec<u8>> {
        self.values.get(path)
    }

    /// Takes and returns the retained sparse node updates
    pub fn take_updates(&mut self) -> SparseTrieUpdates {
        self.updates.take().unwrap_or_default()
    }

    /// Reveal the trie node only if it was not known already.
    pub fn reveal_node(&mut self, path: Nibbles, node: TrieNode) -> SparseTrieResult<()> {
        match node {
            TrieNode::EmptyRoot => {
                debug_assert!(path.is_empty());
                self.nodes.insert(path, SparseNode::Empty);
            }
            TrieNode::Branch(branch) => {
                let mut stack_ptr = branch.as_ref().first_child_index();
                for idx in CHILD_INDEX_RANGE {
                    if branch.state_mask.is_bit_set(idx) {
                        let mut child_path = path.clone();
                        child_path.push_unchecked(idx);
                        self.reveal_node_or_hash(child_path, &branch.stack[stack_ptr])?;
                        stack_ptr += 1;
                    }
                }

                match self.nodes.get(&path) {
                    // Blinded and non-existent nodes can be replaced.
                    Some(SparseNode::Hash(_)) | None => {
                        self.nodes.insert(path, SparseNode::new_branch(branch.state_mask));
                    }
                    // Branch node already exists, or an extension node was placed where a
                    // branch node was before.
                    Some(SparseNode::Branch { .. } | SparseNode::Extension { .. }) => {}
                    // All other node types can't be handled.
                    Some(node @ (SparseNode::Empty | SparseNode::Leaf { .. })) => {
                        return Err(SparseTrieError::Reveal { path, node: Box::new(node.clone()) })
                    }
                }
            }
            TrieNode::Extension(ext) => match self.nodes.get(&path) {
                Some(SparseNode::Hash(_)) | None => {
                    let mut child_path = path.clone();
                    child_path.extend_from_slice_unchecked(&ext.key);
                    self.reveal_node_or_hash(child_path, &ext.child)?;
                    self.nodes.insert(path, SparseNode::new_ext(ext.key));
                }
                // Extension node already exists, or an extension node was placed where a branch
                // node was before.
                Some(SparseNode::Extension { .. } | SparseNode::Branch { .. }) => {}
                // All other node types can't be handled.
                Some(node @ (SparseNode::Empty | SparseNode::Leaf { .. })) => {
                    return Err(SparseTrieError::Reveal { path, node: Box::new(node.clone()) })
                }
            },
            TrieNode::Leaf(leaf) => match self.nodes.get(&path) {
                Some(SparseNode::Hash(_)) | None => {
                    let mut full = path.clone();
                    full.extend_from_slice_unchecked(&leaf.key);
                    self.values.insert(full, leaf.value);
                    self.nodes.insert(path, SparseNode::new_leaf(leaf.key));
                }
                // Left node already exists.
                Some(SparseNode::Leaf { .. }) => {}
                // All other node types can't be handled.
                Some(
                    node @ (SparseNode::Empty |
                    SparseNode::Extension { .. } |
                    SparseNode::Branch { .. }),
                ) => return Err(SparseTrieError::Reveal { path, node: Box::new(node.clone()) }),
            },
        }

        Ok(())
    }

    fn reveal_node_or_hash(&mut self, path: Nibbles, child: &[u8]) -> SparseTrieResult<()> {
        if child.len() == B256::len_bytes() + 1 {
            let hash = B256::from_slice(&child[1..]);
            match self.nodes.get(&path) {
                // Hash node with a different hash can't be handled.
                Some(node @ SparseNode::Hash(previous_hash)) if previous_hash != &hash => {
                    return Err(SparseTrieError::Reveal { path, node: Box::new(node.clone()) })
                }
                None => {
                    self.nodes.insert(path, SparseNode::Hash(hash));
                }
                // All other node types mean that it has already been revealed.
                Some(_) => {}
            }
            return Ok(())
        }

        self.reveal_node(path, TrieNode::decode(&mut &child[..])?)
    }

    /// Update the leaf node with provided value.
    pub fn update_leaf(&mut self, path: Nibbles, value: Vec<u8>) -> SparseTrieResult<()> {
        self.prefix_set.insert(path.clone());
        let existing = self.values.insert(path.clone(), value);
        if existing.is_some() {
            // trie structure unchanged, return immediately
            return Ok(())
        }

        let mut current = Nibbles::default();
        while let Some(node) = self.nodes.get_mut(&current) {
            match node {
                SparseNode::Empty => {
                    *node = SparseNode::new_leaf(path);
                    break
                }
                SparseNode::Hash(hash) => {
                    return Err(SparseTrieError::BlindedNode { path: current, hash: *hash })
                }
                SparseNode::Leaf { key: current_key, .. } => {
                    current.extend_from_slice_unchecked(current_key);

                    // this leaf is being updated
                    if current == path {
                        unreachable!("we already checked leaf presence in the beginning");
                    }

                    // find the common prefix
                    let common = current.common_prefix_length(&path);

                    // update existing node
                    let new_ext_key = current.slice(current.len() - current_key.len()..common);
                    *node = SparseNode::new_ext(new_ext_key);

                    // create a branch node and corresponding leaves
                    self.nodes.insert(
                        current.slice(..common),
                        SparseNode::new_split_branch(current[common], path[common]),
                    );
                    self.nodes.insert(
                        path.slice(..=common),
                        SparseNode::new_leaf(path.slice(common + 1..)),
                    );
                    self.nodes.insert(
                        current.slice(..=common),
                        SparseNode::new_leaf(current.slice(common + 1..)),
                    );

                    break;
                }
                SparseNode::Extension { key, .. } => {
                    current.extend_from_slice(key);
                    if !path.starts_with(&current) {
                        // find the common prefix
                        let common = current.common_prefix_length(&path);

                        *key = current.slice(current.len() - key.len()..common);

                        // create state mask for new branch node
                        // NOTE: this might overwrite the current extension node
                        let branch = SparseNode::new_split_branch(current[common], path[common]);
                        self.nodes.insert(current.slice(..common), branch);

                        // create new leaf
                        let new_leaf = SparseNode::new_leaf(path.slice(common + 1..));
                        self.nodes.insert(path.slice(..=common), new_leaf);

                        // recreate extension to previous child if needed
                        let key = current.slice(common + 1..);
                        if !key.is_empty() {
                            self.nodes.insert(current.slice(..=common), SparseNode::new_ext(key));
                        }

                        break;
                    }
                }
                SparseNode::Branch { state_mask, .. } => {
                    let nibble = path[current.len()];
                    current.push_unchecked(nibble);
                    if !state_mask.is_bit_set(nibble) {
                        state_mask.set_bit(nibble);
                        let new_leaf = SparseNode::new_leaf(path.slice(current.len()..));
                        self.nodes.insert(current, new_leaf);
                        break;
                    }
                }
            };
        }

        Ok(())
    }

    /// Traverse trie nodes down to the leaf node and collect all nodes along the path.
    fn take_nodes_for_path(&mut self, path: &Nibbles) -> SparseTrieResult<Vec<RemovedSparseNode>> {
        let mut current = Nibbles::default(); // Start traversal from the root
        let mut nodes = Vec::new(); // Collect traversed nodes

        while let Some(node) = self.nodes.remove(&current) {
            match &node {
                SparseNode::Empty => return Err(SparseTrieError::Blind),
                SparseNode::Hash(hash) => {
                    return Err(SparseTrieError::BlindedNode { path: current, hash: *hash })
                }
                SparseNode::Leaf { key: _key, .. } => {
                    // Leaf node is always the one that we're deleting, and no other leaf nodes can
                    // be found during traversal.

                    #[cfg(debug_assertions)]
                    {
                        let mut current = current.clone();
                        current.extend_from_slice_unchecked(_key);
                        assert_eq!(&current, path);
                    }

                    nodes.push(RemovedSparseNode {
                        path: current.clone(),
                        node,
                        unset_branch_nibble: None,
                    });
                    break
                }
                SparseNode::Extension { key, .. } => {
                    #[cfg(debug_assertions)]
                    {
                        let mut current = current.clone();
                        current.extend_from_slice_unchecked(key);
                        assert!(
                            path.starts_with(&current),
                            "path: {:?}, current: {:?}, key: {:?}",
                            path,
                            current,
                            key
                        );
                    }

                    let path = current.clone();
                    current.extend_from_slice_unchecked(key);
                    nodes.push(RemovedSparseNode { path, node, unset_branch_nibble: None });
                }
                SparseNode::Branch { state_mask, .. } => {
                    let nibble = path[current.len()];
                    debug_assert!(
                        state_mask.is_bit_set(nibble),
                        "current: {:?}, path: {:?}, nibble: {:?}, state_mask: {:?}",
                        current,
                        path,
                        nibble,
                        state_mask
                    );

                    // If the branch node has a child that is a leaf node that we're removing,
                    // we need to unset this nibble.
                    // Any other branch nodes will not require unsetting the nibble, because
                    // deleting one leaf node can not remove the whole path
                    // where the branch node is located.
                    let mut child_path =
                        Nibbles::from_nibbles([current.as_slice(), &[nibble]].concat());
                    let unset_branch_nibble = self
                        .nodes
                        .get(&child_path)
                        .is_some_and(move |node| match node {
                            SparseNode::Leaf { key, .. } => {
                                // Get full path of the leaf node
                                child_path.extend_from_slice_unchecked(key);
                                &child_path == path
                            }
                            _ => false,
                        })
                        .then_some(nibble);

                    nodes.push(RemovedSparseNode {
                        path: current.clone(),
                        node,
                        unset_branch_nibble,
                    });

                    current.push_unchecked(nibble);
                }
            }
        }

        Ok(nodes)
    }

    /// Wipe the trie, removing all values and nodes, and replacing the root with an empty node.
    pub fn wipe(&mut self) {
        self.nodes = HashMap::from_iter([(Nibbles::default(), SparseNode::Empty)]);
        self.values = HashMap::default();
        self.prefix_set = PrefixSetMut::all();
        self.updates = self.updates.is_some().then(SparseTrieUpdates::wiped);
    }

    /// Return the root of the sparse trie.
    /// Updates all remaining dirty nodes before calculating the root.
    pub fn root(&mut self) -> B256 {
        // take the current prefix set.
        let mut prefix_set = std::mem::take(&mut self.prefix_set).freeze();
        let rlp_node = self.rlp_node_allocate(Nibbles::default(), &mut prefix_set);
        if let Some(root_hash) = rlp_node.as_hash() {
            root_hash
        } else {
            keccak256(rlp_node)
        }
    }

    /// Update hashes of the nodes that are located at a level deeper than or equal to the provided
    /// depth. Root node has a level of 0.
    pub fn update_rlp_node_level(&mut self, depth: usize) {
        let mut prefix_set = self.prefix_set.clone().freeze();
        let mut buffers = RlpNodeBuffers::default();

        let targets = self.get_changed_nodes_at_depth(&mut prefix_set, depth);
        for target in targets {
            buffers.path_stack.push((target, Some(true)));
            self.rlp_node(&mut prefix_set, &mut buffers);
        }
    }

    /// Returns a list of paths to the nodes that were changed according to the prefix set and are
    /// located at the provided depth when counting from the root node. If there's a leaf at a
    /// depth less than the provided depth, it will be included in the result.
    fn get_changed_nodes_at_depth(&self, prefix_set: &mut PrefixSet, depth: usize) -> Vec<Nibbles> {
        let mut paths = Vec::from([(Nibbles::default(), 0)]);
        let mut targets = Vec::new();

        while let Some((mut path, level)) = paths.pop() {
            match self.nodes.get(&path).unwrap() {
                SparseNode::Empty | SparseNode::Hash(_) => {}
                SparseNode::Leaf { hash, .. } => {
                    if hash.is_some() && !prefix_set.contains(&path) {
                        continue
                    }

                    targets.push(path);
                }
                SparseNode::Extension { key, hash } => {
                    if hash.is_some() && !prefix_set.contains(&path) {
                        continue
                    }

                    if level >= depth {
                        targets.push(path);
                    } else {
                        path.extend_from_slice_unchecked(key);
                        paths.push((path, level + 1));
                    }
                }
                SparseNode::Branch { state_mask, hash, .. } => {
                    if hash.is_some() && !prefix_set.contains(&path) {
                        continue
                    }

                    if level >= depth {
                        targets.push(path);
                    } else {
                        for bit in CHILD_INDEX_RANGE.rev() {
                            if state_mask.is_bit_set(bit) {
                                let mut child_path = path.clone();
                                child_path.push_unchecked(bit);
                                paths.push((child_path, level + 1));
                            }
                        }
                    }
                }
            }
        }

        targets
    }

    fn rlp_node_allocate(&mut self, path: Nibbles, prefix_set: &mut PrefixSet) -> RlpNode {
        let mut buffers = RlpNodeBuffers::new_with_path(path);
        self.rlp_node(prefix_set, &mut buffers)
    }

    fn rlp_node(&mut self, prefix_set: &mut PrefixSet, buffers: &mut RlpNodeBuffers) -> RlpNode {
        'main: while let Some((path, mut is_in_prefix_set)) = buffers.path_stack.pop() {
            // Check if the path is in the prefix set.
            // First, check the cached value. If it's `None`, then check the prefix set, and update
            // the cached value.
            let mut prefix_set_contains =
                |path: &Nibbles| *is_in_prefix_set.get_or_insert_with(|| prefix_set.contains(path));

            let (rlp_node, calculated, node_type) = match self.nodes.get_mut(&path).unwrap() {
                SparseNode::Empty => {
                    (RlpNode::word_rlp(&EMPTY_ROOT_HASH), false, SparseNodeType::Empty)
                }
                SparseNode::Hash(hash) => (RlpNode::word_rlp(hash), false, SparseNodeType::Hash),
                SparseNode::Leaf { key, hash } => {
                    self.rlp_buf.clear();
                    let mut path = path.clone();
                    path.extend_from_slice_unchecked(key);
                    if let Some(hash) = hash.filter(|_| !prefix_set_contains(&path)) {
                        (RlpNode::word_rlp(&hash), false, SparseNodeType::Leaf)
                    } else {
                        let value = self.values.get(&path).unwrap();
                        let rlp_node = LeafNodeRef { key, value }.rlp(&mut self.rlp_buf);
                        *hash = rlp_node.as_hash();
                        (rlp_node, true, SparseNodeType::Leaf)
                    }
                }
                SparseNode::Extension { key, hash } => {
                    let mut child_path = path.clone();
                    child_path.extend_from_slice_unchecked(key);
                    if let Some(hash) = hash.filter(|_| !prefix_set_contains(&path)) {
                        (
                            RlpNode::word_rlp(&hash),
                            false,
                            SparseNodeType::Extension { store_in_db_trie: true },
                        )
                    } else if buffers.rlp_node_stack.last().is_some_and(|e| e.0 == child_path) {
                        let (_, child, _, node_type) = buffers.rlp_node_stack.pop().unwrap();
                        self.rlp_buf.clear();
                        let rlp_node = ExtensionNodeRef::new(key, &child).rlp(&mut self.rlp_buf);
                        *hash = rlp_node.as_hash();

                        (
                            rlp_node,
                            true,
                            SparseNodeType::Extension {
                                // Inherit the `store_in_db_trie` flag from the child node, which is
                                // always the branch node
                                store_in_db_trie: node_type.store_in_db_trie(),
                            },
                        )
                    } else {
                        // need to get rlp node for child first
                        buffers.path_stack.extend([(path, is_in_prefix_set), (child_path, None)]);
                        continue
                    }
                }
                SparseNode::Branch { state_mask, hash, store_in_db_trie } => {
                    if let Some((hash, store_in_db_trie)) =
                        hash.zip(*store_in_db_trie).filter(|_| !prefix_set_contains(&path))
                    {
                        buffers.rlp_node_stack.push((
                            path,
                            RlpNode::word_rlp(&hash),
                            false,
                            SparseNodeType::Branch { store_in_db_trie },
                        ));
                        continue
                    }

                    buffers.branch_child_buf.clear();
                    // Walk children in a reverse order from `f` to `0`, so we pop the `0` first
                    // from the stack and keep walking in the sorted order.
                    for bit in CHILD_INDEX_RANGE.rev() {
                        if state_mask.is_bit_set(bit) {
                            let mut child = path.clone();
                            child.push_unchecked(bit);
                            buffers.branch_child_buf.push(child);
                        }
                    }

                    buffers
                        .branch_value_stack_buf
                        .resize(buffers.branch_child_buf.len(), Default::default());
                    let mut added_children = false;

                    // TODO(alexey): set the `TrieMask` bits directly
                    let mut tree_mask_values = Vec::new();
                    let mut hash_mask_values = Vec::new();
                    let mut hashes = Vec::new();
                    for (i, child_path) in buffers.branch_child_buf.iter().enumerate() {
                        if buffers.rlp_node_stack.last().is_some_and(|e| &e.0 == child_path) {
                            let (_, child, calculated, node_type) =
                                buffers.rlp_node_stack.pop().unwrap();

                            // Update the masks only if we need to retain trie updates
                            if self.updates.is_some() {
                                // Set the trie mask
                                if node_type.store_in_db_trie() {
                                    // A branch or an extension node explicitly set the
                                    // `store_in_db_trie` flag
                                    tree_mask_values.push(true);
                                } else {
                                    // Set the flag according to whether a child node was
                                    // pre-calculated
                                    // (`calculated = false`), meaning that it wasn't in the
                                    // database
                                    tree_mask_values.push(!calculated);
                                }

                                // Set the hash mask. If a child node has a hash value AND is a
                                // branch node, set the hash mask and save the hash.
                                let hash = child.as_hash().filter(|_| node_type.is_branch());
                                hash_mask_values.push(hash.is_some());
                                if let Some(hash) = hash {
                                    hashes.push(hash);
                                }
                            }

                            // Insert children in the resulting buffer in a normal order,
                            // because initially we iterated in reverse.
                            buffers.branch_value_stack_buf
                                [buffers.branch_child_buf.len() - i - 1] = child;
                            added_children = true;
                        } else {
                            debug_assert!(!added_children);
                            buffers.path_stack.push((path, is_in_prefix_set));
                            buffers
                                .path_stack
                                .extend(buffers.branch_child_buf.drain(..).map(|p| (p, None)));
                            continue 'main
                        }
                    }

                    self.rlp_buf.clear();
                    let branch_node_ref =
                        BranchNodeRef::new(&buffers.branch_value_stack_buf, *state_mask);
                    let rlp_node = branch_node_ref.rlp(&mut self.rlp_buf);
                    *hash = rlp_node.as_hash();

                    let store_in_db_trie_value = if let Some(updates) = self.updates.as_mut() {
                        let mut tree_mask_values = tree_mask_values.into_iter().rev();
                        let mut hash_mask_values = hash_mask_values.into_iter().rev();
                        let mut tree_mask = TrieMask::default();
                        let mut hash_mask = TrieMask::default();
                        for (i, child) in branch_node_ref.children() {
                            if child.is_some() {
                                if hash_mask_values.next().unwrap() {
                                    hash_mask.set_bit(i);
                                }
                                if tree_mask_values.next().unwrap() {
                                    tree_mask.set_bit(i);
                                }
                            }
                        }

                        // Store in DB trie if there are either any children that are stored in the
                        // DB trie, or any children represent hashed values
                        let store_in_db_trie = !tree_mask.is_empty() || !hash_mask.is_empty();
                        if store_in_db_trie {
                            hashes.reverse();
                            let branch_node = BranchNodeCompact::new(
                                *state_mask,
                                tree_mask,
                                hash_mask,
                                hashes,
                                hash.filter(|_| path.len() == 0),
                            );
                            updates.updated_nodes.insert(path.clone(), branch_node);
                        }

                        store_in_db_trie
                    } else {
                        false
                    };
                    *store_in_db_trie = Some(store_in_db_trie_value);

                    (
                        rlp_node,
                        true,
                        SparseNodeType::Branch { store_in_db_trie: store_in_db_trie_value },
                    )
                }
            };
            buffers.rlp_node_stack.push((path, rlp_node, calculated, node_type));
        }

        debug_assert_eq!(buffers.rlp_node_stack.len(), 1);
        buffers.rlp_node_stack.pop().unwrap().1
    }
}

impl<P> RevealedSparseTrie<P>
where
    P: BlindedProvider,
    SparseTrieError: From<P::Error>,
{
    /// Remove leaf node from the trie.
    pub fn remove_leaf(&mut self, path: &Nibbles) -> SparseTrieResult<()> {
        self.prefix_set.insert(path.clone());
        self.values.remove(path);

        // If the path wasn't present in `values`, we still need to walk the trie and ensure that
        // there is no node at the path. When a leaf node is a blinded `Hash`, it will have an entry
        // in `nodes`, but not in the `values`.

        let mut removed_nodes = self.take_nodes_for_path(path)?;
        trace!(target: "trie::sparse", ?path, ?removed_nodes, "Removed nodes for path");
        // Pop the first node from the stack which is the leaf node we want to remove.
        let mut child = removed_nodes.pop().expect("leaf exists");
        #[cfg(debug_assertions)]
        {
            let mut child_path = child.path.clone();
            let SparseNode::Leaf { key, .. } = &child.node else { panic!("expected leaf node") };
            child_path.extend_from_slice_unchecked(key);
            assert_eq!(&child_path, path);
        }

        // If we don't have any other removed nodes, insert an empty node at the root.
        if removed_nodes.is_empty() {
            debug_assert!(self.nodes.is_empty());
            self.nodes.insert(Nibbles::default(), SparseNode::Empty);

            return Ok(())
        }

        // Walk the stack of removed nodes from the back and re-insert them back into the trie,
        // adjusting the node type as needed.
        while let Some(removed_node) = removed_nodes.pop() {
            let removed_path = removed_node.path;

            let new_node = match &removed_node.node {
                SparseNode::Empty => return Err(SparseTrieError::Blind),
                SparseNode::Hash(hash) => {
                    return Err(SparseTrieError::BlindedNode { path: removed_path, hash: *hash })
                }
                SparseNode::Leaf { .. } => {
                    unreachable!("we already popped the leaf node")
                }
                SparseNode::Extension { key, .. } => {
                    // If the node is an extension node, we need to look at its child to see if we
                    // need to merge them.
                    match &child.node {
                        SparseNode::Empty => return Err(SparseTrieError::Blind),
                        SparseNode::Hash(hash) => {
                            return Err(SparseTrieError::BlindedNode {
                                path: child.path,
                                hash: *hash,
                            })
                        }
                        // For a leaf node, we collapse the extension node into a leaf node,
                        // extending the key. While it's impossible to encounter an extension node
                        // followed by a leaf node in a complete trie, it's possible here because we
                        // could have downgraded the extension node's child into a leaf node from
                        // another node type.
                        SparseNode::Leaf { key: leaf_key, .. } => {
                            self.nodes.remove(&child.path);

                            let mut new_key = key.clone();
                            new_key.extend_from_slice_unchecked(leaf_key);
                            SparseNode::new_leaf(new_key)
                        }
                        // For an extension node, we collapse them into one extension node,
                        // extending the key
                        SparseNode::Extension { key: extension_key, .. } => {
                            self.nodes.remove(&child.path);

                            let mut new_key = key.clone();
                            new_key.extend_from_slice_unchecked(extension_key);
                            SparseNode::new_ext(new_key)
                        }
                        // For a branch node, we just leave the extension node as-is.
                        SparseNode::Branch { .. } => removed_node.node,
                    }
                }
                SparseNode::Branch { mut state_mask, hash: _, store_in_db_trie: _ } => {
                    // If the node is a branch node, we need to check the number of children left
                    // after deleting the child at the given nibble.

                    if let Some(removed_nibble) = removed_node.unset_branch_nibble {
                        state_mask.unset_bit(removed_nibble);
                    }

                    // If only one child is left set in the branch node, we need to collapse it.
                    if state_mask.count_bits() == 1 {
                        let child_nibble =
                            state_mask.first_set_bit_index().expect("state mask is not empty");

                        // Get full path of the only child node left.
                        let mut child_path = removed_path.clone();
                        child_path.push_unchecked(child_nibble);

                        trace!(target: "trie::sparse", ?removed_path, ?child_path, ?child, "Branch node has only one child");

                        if self.nodes.get(&child_path).unwrap().is_hash() {
                            trace!(target: "trie::sparse", ?child_path, "Retrieving remaining blinded branch child");
                            if let Some(node) = self.provider.blinded_node(child_path.clone())? {
                                let decoded = TrieNode::decode(&mut &node[..])?;
                                trace!(target: "trie::sparse", ?child_path, ?decoded, "Revealing remaining blinded branch child");
                                self.reveal_node(child_path.clone(), decoded)?;
                            }
                        }

                        // Get the only child node.
                        let child = self.nodes.get(&child_path).unwrap();

                        let mut delete_child = false;
                        let new_node = match child {
                            SparseNode::Empty => return Err(SparseTrieError::Blind),
                            SparseNode::Hash(hash) => {
                                return Err(SparseTrieError::BlindedNode {
                                    path: child_path,
                                    hash: *hash,
                                })
                            }
                            // If the only child is a leaf node, we downgrade the branch node into a
                            // leaf node, prepending the nibble to the key, and delete the old
                            // child.
                            SparseNode::Leaf { key, .. } => {
                                delete_child = true;

                                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                                new_key.extend_from_slice_unchecked(key);
                                SparseNode::new_leaf(new_key)
                            }
                            // If the only child node is an extension node, we downgrade the branch
                            // node into an even longer extension node, prepending the nibble to the
                            // key, and delete the old child.
                            SparseNode::Extension { key, .. } => {
                                delete_child = true;

                                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                                new_key.extend_from_slice_unchecked(key);
                                SparseNode::new_ext(new_key)
                            }
                            // If the only child is a branch node, we downgrade the current branch
                            // node into a one-nibble extension node.
                            SparseNode::Branch { .. } => {
                                SparseNode::new_ext(Nibbles::from_nibbles_unchecked([child_nibble]))
                            }
                        };

                        if delete_child {
                            self.nodes.remove(&child_path);
                        }

                        if let Some(updates) = self.updates.as_mut() {
                            updates.removed_nodes.insert(removed_path.clone());
                        }

                        new_node
                    }
                    // If more than one child is left set in the branch, we just re-insert it
                    // as-is.
                    else {
                        SparseNode::new_branch(state_mask)
                    }
                }
            };

            child = RemovedSparseNode {
                path: removed_path.clone(),
                node: new_node.clone(),
                unset_branch_nibble: None,
            };
            trace!(target: "trie::sparse", ?removed_path, ?new_node, "Re-inserting the node");
            self.nodes.insert(removed_path, new_node);
        }

        Ok(())
    }
}

/// Enum representing sparse trie node type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SparseNodeType {
    /// Empty trie node.
    Empty,
    /// The hash of the node that was not revealed.
    Hash,
    /// Sparse leaf node.
    Leaf,
    /// Sparse extension node.
    Extension {
        /// A flag indicating whether the extension node should be stored in the database.
        store_in_db_trie: bool,
    },
    /// Sparse branch node.
    Branch {
        /// A flag indicating whether the branch node should be stored in the database.
        store_in_db_trie: bool,
    },
}

impl SparseNodeType {
    const fn is_branch(&self) -> bool {
        matches!(self, Self::Branch { .. })
    }

    const fn store_in_db_trie(&self) -> bool {
        match *self {
            Self::Extension { store_in_db_trie } | Self::Branch { store_in_db_trie } => {
                store_in_db_trie
            }
            _ => false,
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
    /// Sparse leaf node with remaining key suffix.
    Leaf {
        /// Remaining key suffix for the leaf node.
        key: Nibbles,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        hash: Option<B256>,
    },
    /// Sparse extension node with key.
    Extension {
        /// The key slice stored by this extension node.
        key: Nibbles,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        hash: Option<B256>,
    },
    /// Sparse branch node with state mask.
    Branch {
        /// The bitmask representing children present in the branch node.
        state_mask: TrieMask,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        store_in_db_trie: Option<bool>,
    },
}

impl SparseNode {
    /// Create new sparse node from [`TrieNode`].
    pub fn from_node(node: TrieNode) -> Self {
        match node {
            TrieNode::EmptyRoot => Self::Empty,
            TrieNode::Leaf(leaf) => Self::new_leaf(leaf.key),
            TrieNode::Extension(ext) => Self::new_ext(ext.key),
            TrieNode::Branch(branch) => Self::new_branch(branch.state_mask),
        }
    }

    /// Create new [`SparseNode::Branch`] from state mask.
    pub const fn new_branch(state_mask: TrieMask) -> Self {
        Self::Branch { state_mask, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Branch`] with two bits set.
    pub const fn new_split_branch(bit_a: u8, bit_b: u8) -> Self {
        let state_mask = TrieMask::new(
            // set bits for both children
            (1u16 << bit_a) | (1u16 << bit_b),
        );
        Self::Branch { state_mask, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Extension`] from the key slice.
    pub const fn new_ext(key: Nibbles) -> Self {
        Self::Extension { key, hash: None }
    }

    /// Create new [`SparseNode::Leaf`] from leaf key and value.
    pub const fn new_leaf(key: Nibbles) -> Self {
        Self::Leaf { key, hash: None }
    }

    /// Returns `true` if the node is a hash node.
    pub const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash(_))
    }
}

#[derive(Debug)]
struct RemovedSparseNode {
    path: Nibbles,
    node: SparseNode,
    unset_branch_nibble: Option<u8>,
}

/// Collection of reusable buffers for [`RevealedSparseTrie::rlp_node`].
#[derive(Debug, Default)]
struct RlpNodeBuffers {
    /// Stack of paths we need rlp nodes for and whether the path is in the prefix set.
    path_stack: Vec<(Nibbles, Option<bool>)>,
    /// Stack of rlp nodes
    rlp_node_stack: Vec<(Nibbles, RlpNode, bool, SparseNodeType)>,
    /// Reusable branch child path
    branch_child_buf: SmallVec<[Nibbles; 16]>,
    /// Reusable branch value stack
    branch_value_stack_buf: SmallVec<[RlpNode; 16]>,
}

impl RlpNodeBuffers {
    /// Creates a new instance of buffers with the given path on the stack.
    fn new_with_path(path: Nibbles) -> Self {
        Self {
            path_stack: vec![(path, None)],
            rlp_node_stack: Vec::new(),
            branch_child_buf: SmallVec::<[Nibbles; 16]>::new_const(),
            branch_value_stack_buf: SmallVec::<[RlpNode; 16]>::new_const(),
        }
    }
}

/// The aggregation of sparse trie updates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SparseTrieUpdates {
    pub(crate) updated_nodes: HashMap<Nibbles, BranchNodeCompact>,
    pub(crate) removed_nodes: HashSet<Nibbles>,
    pub(crate) wiped: bool,
}

impl SparseTrieUpdates {
    /// Create new wiped sparse trie updates.
    pub fn wiped() -> Self {
        Self { wiped: true, ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::HashSet, U256};
    use alloy_rlp::Encodable;
    use assert_matches::assert_matches;
    use itertools::Itertools;
    use prop::sample::SizeRange;
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb;
    use rand::seq::IteratorRandom;
    use reth_primitives_traits::Account;
    use reth_trie::{
        hashed_cursor::{noop::NoopHashedAccountCursor, HashedPostStateAccountCursor},
        node_iter::{TrieElement, TrieNodeIter},
        trie_cursor::noop::NoopAccountTrieCursor,
        walker::TrieWalker,
        BranchNode, ExtensionNode, HashedPostState, LeafNode, TrieAccount,
    };
    use reth_trie_common::{
        proof::{ProofNodes, ProofRetainer},
        HashBuilder,
    };
    use std::collections::BTreeMap;

    /// Pad nibbles to the length of a B256 hash with zeros on the left.
    fn pad_nibbles_left(nibbles: Nibbles) -> Nibbles {
        let mut base =
            Nibbles::from_nibbles_unchecked(vec![0; B256::len_bytes() * 2 - nibbles.len()]);
        base.extend_from_slice_unchecked(&nibbles);
        base
    }

    /// Pad nibbles to the length of a B256 hash with zeros on the right.
    fn pad_nibbles_right(mut nibbles: Nibbles) -> Nibbles {
        nibbles.extend_from_slice_unchecked(&vec![0; B256::len_bytes() * 2 - nibbles.len()]);
        nibbles
    }

    /// Calculate the state root by feeding the provided state to the hash builder and retaining the
    /// proofs for the provided targets.
    ///
    /// Returns the state root and the retained proof nodes.
    fn run_hash_builder(
        state: impl IntoIterator<Item = (Nibbles, Account)> + Clone,
        proof_targets: impl IntoIterator<Item = Nibbles>,
    ) -> HashBuilder {
        let mut account_rlp = Vec::new();

        let mut hash_builder = HashBuilder::default()
            .with_updates(true)
            .with_proof_retainer(ProofRetainer::from_iter(proof_targets));

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.extend_keys(state.clone().into_iter().map(|(nibbles, _)| nibbles));
        let walker = TrieWalker::new(NoopAccountTrieCursor::default(), prefix_set.freeze())
            .with_deletions_retained(true);
        let hashed_post_state = HashedPostState::default()
            .with_accounts(state.into_iter().map(|(nibbles, account)| {
                (nibbles.pack().into_inner().unwrap().into(), Some(account))
            }))
            .into_sorted();
        let mut node_iter = TrieNodeIter::new(
            walker,
            HashedPostStateAccountCursor::new(
                NoopHashedAccountCursor::default(),
                hashed_post_state.accounts(),
            ),
        );

        while let Some(node) = node_iter.try_next().unwrap() {
            match node {
                TrieElement::Branch(branch) => {
                    hash_builder.add_branch(branch.key, branch.value, branch.children_are_in_trie);
                }
                TrieElement::Leaf(key, account) => {
                    let account = TrieAccount::from((account, EMPTY_ROOT_HASH));
                    account.encode(&mut account_rlp);

                    hash_builder.add_leaf(Nibbles::unpack(key), &account_rlp);
                    account_rlp.clear();
                }
            }
        }
        hash_builder.root();

        hash_builder
    }

    /// Assert that the sparse trie nodes and the proof nodes from the hash builder are equal.
    fn assert_eq_sparse_trie_proof_nodes(
        sparse_trie: &RevealedSparseTrie,
        proof_nodes: ProofNodes,
    ) {
        let proof_nodes = proof_nodes
            .into_nodes_sorted()
            .into_iter()
            .map(|(path, node)| (path, TrieNode::decode(&mut node.as_ref()).unwrap()));

        let sparse_nodes = sparse_trie.nodes.iter().sorted_by_key(|(path, _)| *path);

        for ((proof_node_path, proof_node), (sparse_node_path, sparse_node)) in
            proof_nodes.zip(sparse_nodes)
        {
            assert_eq!(&proof_node_path, sparse_node_path);

            let equals = match (&proof_node, &sparse_node) {
                // Both nodes are empty
                (TrieNode::EmptyRoot, SparseNode::Empty) => true,
                // Both nodes are branches and have the same state mask
                (
                    TrieNode::Branch(BranchNode { state_mask: proof_state_mask, .. }),
                    SparseNode::Branch { state_mask: sparse_state_mask, .. },
                ) => proof_state_mask == sparse_state_mask,
                // Both nodes are extensions and have the same key
                (
                    TrieNode::Extension(ExtensionNode { key: proof_key, .. }),
                    SparseNode::Extension { key: sparse_key, .. },
                ) |
                // Both nodes are leaves and have the same key
                (
                    TrieNode::Leaf(LeafNode { key: proof_key, .. }),
                    SparseNode::Leaf { key: sparse_key, .. },
                ) => proof_key == sparse_key,
                // Empty and hash nodes are specific to the sparse trie, skip them
                (_, SparseNode::Empty | SparseNode::Hash(_)) => continue,
                _ => false,
            };
            assert!(equals, "proof node: {:?}, sparse node: {:?}", proof_node, sparse_node);
        }
    }

    #[test]
    fn sparse_trie_is_blind() {
        assert!(SparseTrie::blind().is_blind());
        assert!(!SparseTrie::revealed_empty().is_blind());
    }

    #[test]
    fn sparse_trie_empty_update_one() {
        let key = Nibbles::unpack(B256::with_last_byte(42));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        let mut hash_builder = run_hash_builder([(key.clone(), value())], [key.clone()]);

        let mut sparse = RevealedSparseTrie::default().with_updates(true);
        sparse.update_leaf(key, value_encoded()).unwrap();
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder.root());
        assert_eq!(sparse_updates.updated_nodes, hash_builder.updated_branch_nodes.take().unwrap());
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder.take_proof_nodes());
    }

    #[test]
    fn sparse_trie_empty_update_multiple_lower_nibbles() {
        reth_tracing::init_test_tracing();

        let paths = (0..=16).map(|b| Nibbles::unpack(B256::with_last_byte(b))).collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        let mut hash_builder = run_hash_builder(
            paths.iter().cloned().zip(std::iter::repeat_with(value)),
            paths.clone(),
        );

        let mut sparse = RevealedSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(path.clone(), value_encoded()).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder.root());
        assert_eq!(sparse_updates.updated_nodes, hash_builder.updated_branch_nodes.take().unwrap());
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder.take_proof_nodes());
    }

    #[test]
    fn sparse_trie_empty_update_multiple_upper_nibbles() {
        let paths = (239..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        let mut hash_builder = run_hash_builder(
            paths.iter().cloned().zip(std::iter::repeat_with(value)),
            paths.clone(),
        );

        let mut sparse = RevealedSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(path.clone(), value_encoded()).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder.root());
        assert_eq!(sparse_updates.updated_nodes, hash_builder.updated_branch_nodes.take().unwrap());
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder.take_proof_nodes());
    }

    #[test]
    fn sparse_trie_empty_update_multiple() {
        let paths = (0..=255)
            .map(|b| {
                Nibbles::unpack(if b % 2 == 0 {
                    B256::repeat_byte(b)
                } else {
                    B256::with_last_byte(b)
                })
            })
            .collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        let mut hash_builder = run_hash_builder(
            paths.iter().sorted_unstable().cloned().zip(std::iter::repeat_with(value)),
            paths.clone(),
        );

        let mut sparse = RevealedSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(path.clone(), value_encoded()).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder.root());
        pretty_assertions::assert_eq!(
            BTreeMap::from_iter(sparse_updates.updated_nodes),
            BTreeMap::from_iter(hash_builder.updated_branch_nodes.take().unwrap())
        );
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder.take_proof_nodes());
    }

    #[test]
    fn sparse_trie_empty_update_repeated() {
        let paths = (0..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let old_value = Account { nonce: 1, ..Default::default() };
        let old_value_encoded = {
            let mut account_rlp = Vec::new();
            TrieAccount::from((old_value, EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };
        let new_value = Account { nonce: 2, ..Default::default() };
        let new_value_encoded = {
            let mut account_rlp = Vec::new();
            TrieAccount::from((new_value, EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        let mut hash_builder = run_hash_builder(
            paths.iter().cloned().zip(std::iter::repeat_with(|| old_value)),
            paths.clone(),
        );

        let mut sparse = RevealedSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(path.clone(), old_value_encoded.clone()).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.updates_ref();

        assert_eq!(sparse_root, hash_builder.root());
        assert_eq!(sparse_updates.updated_nodes, hash_builder.updated_branch_nodes.take().unwrap());
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder.take_proof_nodes());

        let mut hash_builder = run_hash_builder(
            paths.iter().cloned().zip(std::iter::repeat_with(|| new_value)),
            paths.clone(),
        );

        for path in &paths {
            sparse.update_leaf(path.clone(), new_value_encoded.clone()).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder.root());
        assert_eq!(sparse_updates.updated_nodes, hash_builder.updated_branch_nodes.take().unwrap());
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder.take_proof_nodes());
    }

    #[test]
    fn sparse_trie_remove_leaf() {
        reth_tracing::init_test_tracing();

        let mut sparse = RevealedSparseTrie::default();

        let value = alloy_rlp::encode_fixed_size(&U256::ZERO).to_vec();

        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone())
            .unwrap();
        sparse.update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value).unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1011)
        //     ├── 0 -> Extension (Key = 23)
        //     │        └── Branch (Mask = 0101)
        //     │              ├── 1 -> Leaf (Key = 1, Path = 50231)
        //     │              └── 3 -> Leaf (Key = 3, Path = 50233)
        //     ├── 2 -> Leaf (Key = 013, Path = 52013)
        //     └── 3 -> Branch (Mask = 0101)
        //                ├── 1 -> Leaf (Key = 3102, Path = 53102)
        //                └── 3 -> Branch (Mask = 1010)
        //                       ├── 0 -> Leaf (Key = 3302, Path = 53302)
        //                       └── 2 -> Leaf (Key = 3320, Path = 53320)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1101.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x2, 0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3]),
                    SparseNode::new_branch(0b1010.into())
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]),
                    SparseNode::new_leaf(Nibbles::default())
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]),
                    SparseNode::new_leaf(Nibbles::default())
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x2]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0, 0x1, 0x3]))
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_branch(0b1010.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x1]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0, 0x2]))
                ),
                (Nibbles::from_nibbles([0x5, 0x3, 0x3]), SparseNode::new_branch(0b0101.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0]))
                )
            ])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3])).unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Extension (Key = 23)
        //     │        └── Branch (Mask = 0101)
        //     │              ├── 1 -> Leaf (Key = 0231, Path = 50231)
        //     │              └── 3 -> Leaf (Key = 0233, Path = 50233)
        //     └── 3 -> Branch (Mask = 0101)
        //                ├── 1 -> Leaf (Key = 3102, Path = 53102)
        //                └── 3 -> Branch (Mask = 1010)
        //                       ├── 0 -> Leaf (Key = 3302, Path = 53302)
        //                       └── 2 -> Leaf (Key = 3320, Path = 53320)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x2, 0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3]),
                    SparseNode::new_branch(0b1010.into())
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]),
                    SparseNode::new_leaf(Nibbles::default())
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]),
                    SparseNode::new_leaf(Nibbles::default())
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_branch(0b1010.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x1]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0, 0x2]))
                ),
                (Nibbles::from_nibbles([0x5, 0x3, 0x3]), SparseNode::new_branch(0b0101.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0]))
                )
            ])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1])).unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Leaf (Key = 0233, Path = 50233)
        //     └── 3 -> Branch (Mask = 0101)
        //                ├── 1 -> Leaf (Key = 3102, Path = 53102)
        //                └── 3 -> Branch (Mask = 1010)
        //                       ├── 0 -> Leaf (Key = 3302, Path = 53302)
        //                       └── 2 -> Leaf (Key = 3320, Path = 53320)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2, 0x3, 0x3]))
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_branch(0b1010.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x1]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0, 0x2]))
                ),
                (Nibbles::from_nibbles([0x5, 0x3, 0x3]), SparseNode::new_branch(0b0101.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0]))
                )
            ])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2])).unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Leaf (Key = 0233, Path = 50233)
        //     └── 3 -> Branch (Mask = 1010)
        //                ├── 0 -> Leaf (Key = 3302, Path = 53302)
        //                └── 2 -> Leaf (Key = 3320, Path = 53320)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2, 0x3, 0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x3]))
                ),
                (Nibbles::from_nibbles([0x5, 0x3, 0x3]), SparseNode::new_branch(0b0101.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x0]))
                )
            ])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0])).unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Leaf (Key = 0233, Path = 50233)
        //     └── 3 -> Leaf (Key = 3302, Path = 53302)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into())),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x2, 0x3, 0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3]),
                    SparseNode::new_leaf(Nibbles::from_nibbles([0x3, 0x0, 0x2]))
                ),
            ])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3])).unwrap();

        // Leaf (Key = 53302)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([(
                Nibbles::default(),
                SparseNode::new_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]))
            ),])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2])).unwrap();

        // Empty
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([(Nibbles::default(), SparseNode::Empty)])
        );
    }

    #[test]
    fn sparse_trie_remove_leaf_blinded() {
        let leaf = LeafNode::new(
            Nibbles::default(),
            alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec(),
        );
        let branch = TrieNode::Branch(BranchNode::new(
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)),
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(),
            ],
            TrieMask::new(0b11),
        ));

        let mut sparse = RevealedSparseTrie::from_root(branch.clone(), false).unwrap();

        // Reveal a branch node and one of its children
        //
        // Branch (Mask = 11)
        // ├── 0 -> Hash (Path = 0)
        // └── 1 -> Leaf (Path = 1)
        sparse.reveal_node(Nibbles::default(), branch).unwrap();
        sparse.reveal_node(Nibbles::from_nibbles([0x1]), TrieNode::Leaf(leaf)).unwrap();

        // Removing a blinded leaf should result in an error
        assert_matches!(
            sparse.remove_leaf(&Nibbles::from_nibbles([0x0])),
            Err(SparseTrieError::BlindedNode { path, hash }) if path == Nibbles::from_nibbles([0x0]) && hash == B256::repeat_byte(1)
        );
    }

    #[allow(clippy::type_complexity)]
    #[test]
    fn sparse_trie_fuzz() {
        // Having only the first 3 nibbles set, we narrow down the range of keys
        // to 4096 different hashes. It allows us to generate collisions more likely
        // to test the sparse trie updates.
        const KEY_NIBBLES_LEN: usize = 3;

        fn test(updates: Vec<(HashMap<Nibbles, Account>, HashSet<Nibbles>)>) {
            {
                let mut state = BTreeMap::default();
                let mut sparse = RevealedSparseTrie::default().with_updates(true);

                for (update, keys_to_delete) in updates {
                    // Insert state updates into the sparse trie and calculate the root
                    for (key, account) in update.clone() {
                        let account = TrieAccount::from((account, EMPTY_ROOT_HASH));
                        let mut account_rlp = Vec::new();
                        account.encode(&mut account_rlp);
                        sparse.update_leaf(key, account_rlp).unwrap();
                    }
                    // We need to clone the sparse trie, so that all updated branch nodes are
                    // preserved, and not only those that were changed after the last call to
                    // `root()`.
                    let mut updated_sparse = sparse.clone();
                    let sparse_root = updated_sparse.root();
                    let sparse_updates = updated_sparse.take_updates();

                    // Insert state updates into the hash builder and calculate the root
                    state.extend(update);
                    let mut hash_builder =
                        run_hash_builder(state.clone(), state.keys().cloned().collect::<Vec<_>>());

                    // Assert that the sparse trie root matches the hash builder root
                    assert_eq!(sparse_root, hash_builder.root());
                    // Assert that the sparse trie updates match the hash builder updates
                    pretty_assertions::assert_eq!(
                        sparse_updates.updated_nodes,
                        hash_builder.updated_branch_nodes.take().unwrap()
                    );
                    // Assert that the sparse trie nodes match the hash builder proof nodes
                    assert_eq_sparse_trie_proof_nodes(
                        &updated_sparse,
                        hash_builder.take_proof_nodes(),
                    );

                    // Delete some keys from both the hash builder and the sparse trie and check
                    // that the sparse trie root still matches the hash builder root
                    for key in keys_to_delete {
                        state.remove(&key).unwrap();
                        sparse.remove_leaf(&key).unwrap();
                    }

                    // We need to clone the sparse trie, so that all updated branch nodes are
                    // preserved, and not only those that were changed after the last call to
                    // `root()`.
                    let mut updated_sparse = sparse.clone();
                    let sparse_root = updated_sparse.root();
                    let sparse_updates = updated_sparse.take_updates();

                    let mut hash_builder =
                        run_hash_builder(state.clone(), state.keys().cloned().collect::<Vec<_>>());

                    // Assert that the sparse trie root matches the hash builder root
                    assert_eq!(sparse_root, hash_builder.root());
                    // Assert that the sparse trie updates match the hash builder updates
                    pretty_assertions::assert_eq!(
                        sparse_updates.updated_nodes,
                        hash_builder.updated_branch_nodes.take().unwrap()
                    );
                    // Assert that the sparse trie nodes match the hash builder proof nodes
                    assert_eq_sparse_trie_proof_nodes(
                        &updated_sparse,
                        hash_builder.take_proof_nodes(),
                    );
                }
            }
        }

        fn transform_updates(
            updates: Vec<HashMap<Nibbles, Account>>,
            mut rng: impl Rng,
        ) -> Vec<(HashMap<Nibbles, Account>, HashSet<Nibbles>)> {
            let mut keys = HashSet::new();
            updates
                .into_iter()
                .map(|update| {
                    keys.extend(update.keys().cloned());

                    let keys_to_delete_len = update.len() / 2;
                    let keys_to_delete = (0..keys_to_delete_len)
                        .map(|_| {
                            let key = keys.iter().choose(&mut rng).unwrap().clone();
                            keys.take(&key).unwrap()
                        })
                        .collect();

                    (update, keys_to_delete)
                })
                .collect::<Vec<_>>()
        }

        proptest!(ProptestConfig::with_cases(10), |(
            updates in proptest::collection::vec(
                proptest::collection::hash_map(
                    any_with::<Nibbles>(SizeRange::new(KEY_NIBBLES_LEN..=KEY_NIBBLES_LEN)).prop_map(pad_nibbles_right),
                    arb::<Account>(),
                    1..100,
                ).prop_map(HashMap::from_iter),
                1..100,
            ).prop_perturb(transform_updates)
        )| {
            test(updates)
        });
    }

    /// We have three leaves that share the same prefix: 0x00, 0x01 and 0x02. Hash builder trie has
    /// only nodes 0x00 and 0x01, and we have proofs for them. Node B is new and inserted in the
    /// sparse trie first.
    ///
    /// 1. Reveal the hash builder proof to leaf 0x00 in the sparse trie.
    /// 2. Insert leaf 0x01 into the sparse trie.
    /// 3. Reveal the hash builder proof to leaf 0x02 in the sparse trie.
    ///
    /// The hash builder proof to the leaf 0x02 didn't have the leaf 0x01 at the corresponding
    /// nibble of the branch node, so we need to adjust the branch node instead of fully
    /// replacing it.
    #[test]
    fn sparse_trie_reveal_node_1() {
        let key1 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00]));
        let key2 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01]));
        let key3 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x02]));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        // Generate the proof for the root node and initialize the sparse trie with it
        let proof_nodes =
            run_hash_builder([(key1(), value()), (key3(), value())], [Nibbles::default()])
                .take_proof_nodes();
        let mut sparse = RevealedSparseTrie::from_root(
            TrieNode::decode(&mut &proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            false,
        )
        .unwrap();

        // Generate the proof for the first key and reveal it in the sparse trie
        let proof_nodes =
            run_hash_builder([(key1(), value()), (key3(), value())], [key1()]).take_proof_nodes();
        for (path, node) in proof_nodes.nodes_sorted() {
            sparse.reveal_node(path, TrieNode::decode(&mut &node[..]).unwrap()).unwrap();
        }

        // Check that the branch node exists with only two nibbles set
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b101.into()))
        );

        // Insert the leaf for the second key
        sparse.update_leaf(key2(), value_encoded()).unwrap();

        // Check that the branch node was updated and another nibble was set
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b111.into()))
        );

        // Generate the proof for the third key and reveal it in the sparse trie
        let proof_nodes_3 =
            run_hash_builder([(key1(), value()), (key3(), value())], [key3()]).take_proof_nodes();
        for (path, node) in proof_nodes_3.nodes_sorted() {
            sparse.reveal_node(path, TrieNode::decode(&mut &node[..]).unwrap()).unwrap();
        }

        // Check that nothing changed in the branch node
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b111.into()))
        );

        // Generate the nodes for the full trie with all three key using the hash builder, and
        // compare them to the sparse trie
        let proof_nodes = run_hash_builder(
            [(key1(), value()), (key2(), value()), (key3(), value())],
            [key1(), key2(), key3()],
        )
        .take_proof_nodes();

        assert_eq_sparse_trie_proof_nodes(&sparse, proof_nodes);
    }

    /// We have three leaves: 0x0000, 0x0101, and 0x0102. Hash builder trie has all nodes, and we
    /// have proofs for them.
    ///
    /// 1. Reveal the hash builder proof to leaf 0x00 in the sparse trie.
    /// 2. Remove leaf 0x00 from the sparse trie (that will remove the branch node and create an
    ///    extension node with the key 0x0000).
    /// 3. Reveal the hash builder proof to leaf 0x0101 in the sparse trie.
    ///
    /// The hash builder proof to the leaf 0x0101 had a branch node in the path, but we turned it
    /// into an extension node, so it should ignore this node.
    #[test]
    fn sparse_trie_reveal_node_2() {
        let key1 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00, 0x00]));
        let key2 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01, 0x01]));
        let key3 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01, 0x02]));
        let value = || Account::default();

        // Generate the proof for the root node and initialize the sparse trie with it
        let proof_nodes = run_hash_builder(
            [(key1(), value()), (key2(), value()), (key3(), value())],
            [Nibbles::default()],
        )
        .take_proof_nodes();
        let mut sparse = RevealedSparseTrie::from_root(
            TrieNode::decode(&mut &proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            false,
        )
        .unwrap();

        // Generate the proof for the children of the root branch node and reveal it in the sparse
        // trie
        let proof_nodes = run_hash_builder(
            [(key1(), value()), (key2(), value()), (key3(), value())],
            [key1(), Nibbles::from_nibbles_unchecked([0x01])],
        )
        .take_proof_nodes();
        for (path, node) in proof_nodes.nodes_sorted() {
            sparse.reveal_node(path, TrieNode::decode(&mut &node[..]).unwrap()).unwrap();
        }

        // Check that the branch node exists
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b11.into()))
        );

        // Remove the leaf for the first key
        sparse.remove_leaf(&key1()).unwrap();

        // Check that the branch node was turned into an extension node
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x01])))
        );

        // Generate the proof for the third key and reveal it in the sparse trie
        let proof_nodes =
            run_hash_builder([(key1(), value()), (key2(), value()), (key3(), value())], [key2()])
                .take_proof_nodes();
        for (path, node) in proof_nodes.nodes_sorted() {
            sparse.reveal_node(path, TrieNode::decode(&mut &node[..]).unwrap()).unwrap();
        }

        // Check that nothing changed in the extension node
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x01])))
        );
    }

    /// We have two leaves that share the same prefix: 0x0001 and 0x0002, and a leaf with a
    /// different prefix: 0x0100. Hash builder trie has only the first two leaves, and we have
    /// proofs for them.
    ///
    /// 1. Insert the leaf 0x0100 into the sparse trie, and check that the root extensino node was
    ///    turned into a branch node.
    /// 2. Reveal the leaf 0x0001 in the sparse trie, and check that the root branch node wasn't
    ///    overwritten with the extension node from the proof.
    #[test]
    fn sparse_trie_reveal_node_3() {
        let key1 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00, 0x01]));
        let key2 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00, 0x02]));
        let key3 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01, 0x00]));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        // Generate the proof for the root node and initialize the sparse trie with it
        let proof_nodes =
            run_hash_builder([(key1(), value()), (key2(), value())], [Nibbles::default()])
                .take_proof_nodes();
        let mut sparse = RevealedSparseTrie::from_root(
            TrieNode::decode(&mut &proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            false,
        )
        .unwrap();

        // Check that the root extension node exists
        assert_matches!(
            sparse.nodes.get(&Nibbles::default()),
            Some(SparseNode::Extension { key, hash: None }) if *key == Nibbles::from_nibbles([0x00])
        );

        // Insert the leaf with a different prefix
        sparse.update_leaf(key3(), value_encoded()).unwrap();

        // Check that the extension node was turned into a branch node
        assert_matches!(
            sparse.nodes.get(&Nibbles::default()),
            Some(SparseNode::Branch { state_mask, hash: None, store_in_db_trie: None }) if *state_mask == TrieMask::new(0b11)
        );

        // Generate the proof for the first key and reveal it in the sparse trie
        let proof_nodes =
            run_hash_builder([(key1(), value()), (key2(), value())], [key1()]).take_proof_nodes();
        for (path, node) in proof_nodes.nodes_sorted() {
            sparse.reveal_node(path, TrieNode::decode(&mut &node[..]).unwrap()).unwrap();
        }

        // Check that the branch node wasn't overwritten by the extension node in the proof
        assert_matches!(
            sparse.nodes.get(&Nibbles::default()),
            Some(SparseNode::Branch { state_mask, hash: None, store_in_db_trie: None }) if *state_mask == TrieMask::new(0b11)
        );
    }

    #[test]
    fn sparse_trie_get_changed_nodes_at_depth() {
        let mut sparse = RevealedSparseTrie::default();

        let value = alloy_rlp::encode_fixed_size(&U256::ZERO).to_vec();

        // Extension (Key = 5) – Level 0
        // └── Branch (Mask = 1011) – Level 1
        //     ├── 0 -> Extension (Key = 23) – Level 2
        //     │        └── Branch (Mask = 0101) – Level 3
        //     │              ├── 1 -> Leaf (Key = 1, Path = 50231) – Level 4
        //     │              └── 3 -> Leaf (Key = 3, Path = 50233) – Level 4
        //     ├── 2 -> Leaf (Key = 013, Path = 52013) – Level 2
        //     └── 3 -> Branch (Mask = 0101) – Level 2
        //                ├── 1 -> Leaf (Key = 3102, Path = 53102) – Level 3
        //                └── 3 -> Branch (Mask = 1010) – Level 3
        //                       ├── 0 -> Leaf (Key = 3302, Path = 53302) – Level 4
        //                       └── 2 -> Leaf (Key = 3320, Path = 53320) – Level 4
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone())
            .unwrap();
        sparse.update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value).unwrap();

        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 0),
            vec![Nibbles::default()]
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 1),
            vec![Nibbles::from_nibbles_unchecked([0x5])]
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 2),
            vec![
                Nibbles::from_nibbles_unchecked([0x5, 0x0]),
                Nibbles::from_nibbles_unchecked([0x5, 0x2]),
                Nibbles::from_nibbles_unchecked([0x5, 0x3])
            ]
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 3),
            vec![
                Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3]),
                Nibbles::from_nibbles_unchecked([0x5, 0x2]),
                Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x1]),
                Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3])
            ]
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 4),
            vec![
                Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3, 0x1]),
                Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3, 0x3]),
                Nibbles::from_nibbles_unchecked([0x5, 0x2]),
                Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x1]),
                Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3, 0x0]),
                Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3, 0x2])
            ]
        );
    }

    #[test]
    fn hash_builder_branch_hash_mask() {
        let key1 = || pad_nibbles_left(Nibbles::from_nibbles_unchecked([0x00]));
        let key2 = || pad_nibbles_left(Nibbles::from_nibbles_unchecked([0x01]));
        let value = || Account { bytecode_hash: Some(B256::repeat_byte(1)), ..Default::default() };
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            TrieAccount::from((value(), EMPTY_ROOT_HASH)).encode(&mut account_rlp);
            account_rlp
        };

        let mut hash_builder =
            run_hash_builder([(key1(), value()), (key2(), value())], [Nibbles::default()]);
        let mut sparse = RevealedSparseTrie::default();
        sparse.update_leaf(key1(), value_encoded()).unwrap();
        sparse.update_leaf(key2(), value_encoded()).unwrap();
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder.root());
        assert_eq!(sparse_updates.updated_nodes, hash_builder.updated_branch_nodes.take().unwrap());
    }

    #[test]
    fn sparse_trie_wipe() {
        let mut sparse = RevealedSparseTrie::default().with_updates(true);

        let value = alloy_rlp::encode_fixed_size(&U256::ZERO).to_vec();

        // Extension (Key = 5) – Level 0
        // └── Branch (Mask = 1011) – Level 1
        //     ├── 0 -> Extension (Key = 23) – Level 2
        //     │        └── Branch (Mask = 0101) – Level 3
        //     │              ├── 1 -> Leaf (Key = 1, Path = 50231) – Level 4
        //     │              └── 3 -> Leaf (Key = 3, Path = 50233) – Level 4
        //     ├── 2 -> Leaf (Key = 013, Path = 52013) – Level 2
        //     └── 3 -> Branch (Mask = 0101) – Level 2
        //                ├── 1 -> Leaf (Key = 3102, Path = 53102) – Level 3
        //                └── 3 -> Branch (Mask = 1010) – Level 3
        //                       ├── 0 -> Leaf (Key = 3302, Path = 53302) – Level 4
        //                       └── 2 -> Leaf (Key = 3320, Path = 53320) – Level 4
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone())
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone())
            .unwrap();
        sparse.update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value).unwrap();

        sparse.wipe();

        assert_eq!(sparse.root(), EMPTY_ROOT_HASH);
    }
}
