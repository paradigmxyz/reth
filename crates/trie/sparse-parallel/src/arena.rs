//! Arena-based parallel sparse trie implementation.
//!
//! This module provides an alternative implementation of the parallel sparse trie that uses
//! arena allocation (via `generational-arena`) instead of `HashMap`s for node storage. The goal
//! is to reduce lookup overhead by tracking nodes by arena IDs rather than path-based `HashMap`
//! lookups.

use crate::{ParallelismThresholds, SparseSubtrieType, NUM_LOWER_SUBTRIES, UPPER_TRIE_MAX_DEPTH};
use alloc::{borrow::Cow, vec::Vec};
use alloy_primitives::{map::HashMap, B256};
use alloy_rlp::Decodable;
use alloy_trie::{TrieMask, EMPTY_ROOT_HASH};
use generational_arena::{Arena, Index};
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{
    prefix_set::PrefixSetMut, BranchNodeMasks, BranchNodeMasksMap, Nibbles, ProofTrieNode, TrieNode,
};
use reth_trie_sparse::{
    LeafLookup, LeafLookupError, LeafUpdate, SparseNodeType, SparseTrie, SparseTrieExt,
    SparseTrieUpdates,
};

/// Type alias for arena node indices.
pub type NodeId = Index;

/// Enum representing trie nodes in the arena-based sparse trie.
///
/// Unlike [`reth_trie_sparse::SparseNode`], this enum stores child references as arena indices
/// rather than relying on path-based `HashMap` lookups.
///
/// Note: The `Branch` variant is intentionally large (contains `[Option<NodeId>; 16]` inline)
/// for O(1) child access without indirection, which is the key optimization being tested.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ArenaSparseNode {
    /// Empty trie node.
    Empty,
    /// The hash of a node that was not revealed.
    Hash(B256),
    /// Sparse leaf node with remaining key suffix.
    Leaf {
        /// Remaining key suffix for the leaf node.
        key: Nibbles,
        /// Pre-computed hash of the sparse node.
        hash: Option<B256>,
    },
    /// Sparse extension node with key and child pointer.
    Extension {
        /// The key slice stored by this extension node.
        key: Nibbles,
        /// Arena ID of the child node.
        child: NodeId,
        /// Pre-computed hash of the sparse node.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        store_in_db_trie: Option<bool>,
    },
    /// Sparse branch node with state mask and child pointers.
    Branch {
        /// The bitmask representing children present in the branch node.
        state_mask: TrieMask,
        /// Direct pointers to child nodes (indexed by nibble 0-15).
        children: [Option<NodeId>; 16],
        /// Pre-computed hash of the sparse node.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        store_in_db_trie: Option<bool>,
    },
}

impl ArenaSparseNode {
    /// Create new sparse node from [`TrieNode`].
    ///
    /// Note: For Branch and Extension nodes, children are initialized as None/placeholder.
    /// The caller must set up child references after insertion into the arena.
    pub fn from_node(node: TrieNode) -> Self {
        match node {
            TrieNode::EmptyRoot => Self::Empty,
            TrieNode::Leaf(leaf) => Self::new_leaf(leaf.key),
            TrieNode::Extension(ext) => Self::new_ext_placeholder(ext.key),
            TrieNode::Branch(branch) => Self::new_branch(branch.state_mask),
        }
    }

    /// Create new [`ArenaSparseNode::Branch`] from state mask with no children set.
    pub const fn new_branch(state_mask: TrieMask) -> Self {
        Self::Branch { state_mask, children: [None; 16], hash: None, store_in_db_trie: None }
    }

    /// Create new [`ArenaSparseNode::Branch`] with two bits set.
    pub const fn new_split_branch(bit_a: u8, bit_b: u8) -> Self {
        let state_mask = TrieMask::new((1u16 << bit_a) | (1u16 << bit_b));
        Self::Branch { state_mask, children: [None; 16], hash: None, store_in_db_trie: None }
    }

    /// Create new [`ArenaSparseNode::Extension`] with a placeholder child.
    ///
    /// The child must be set after the node and its child are inserted into the arena.
    pub fn new_ext_placeholder(key: Nibbles) -> Self {
        Self::Extension {
            key,
            child: Index::from_raw_parts(usize::MAX, u64::MAX),
            hash: None,
            store_in_db_trie: None,
        }
    }

    /// Create new [`ArenaSparseNode::Extension`] with a known child.
    pub const fn new_ext(key: Nibbles, child: NodeId) -> Self {
        Self::Extension { key, child, hash: None, store_in_db_trie: None }
    }

    /// Create new [`ArenaSparseNode::Leaf`] from leaf key.
    pub const fn new_leaf(key: Nibbles) -> Self {
        Self::Leaf { key, hash: None }
    }

    /// Returns `true` if the node is a hash node.
    pub const fn is_hash(&self) -> bool {
        matches!(self, Self::Hash(_))
    }

    /// Returns the hash of the node if it exists.
    pub const fn hash(&self) -> Option<B256> {
        match self {
            Self::Empty => None,
            Self::Hash(hash) => Some(*hash),
            Self::Leaf { hash, .. } | Self::Extension { hash, .. } | Self::Branch { hash, .. } => {
                *hash
            }
        }
    }

    /// Returns the type of this node.
    pub const fn node_type(&self) -> SparseNodeType {
        match self {
            Self::Empty => SparseNodeType::Empty,
            Self::Hash(_) => SparseNodeType::Hash,
            Self::Leaf { .. } => SparseNodeType::Leaf,
            Self::Extension { store_in_db_trie, .. } => {
                SparseNodeType::Extension { store_in_db_trie: *store_in_db_trie }
            }
            Self::Branch { store_in_db_trie, .. } => {
                SparseNodeType::Branch { store_in_db_trie: *store_in_db_trie }
            }
        }
    }

    /// Get the child at the given nibble index for branch nodes.
    pub const fn branch_child(&self, nibble: u8) -> Option<NodeId> {
        match self {
            Self::Branch { children, .. } => children[nibble as usize],
            _ => None,
        }
    }

    /// Set the child at the given nibble index for branch nodes.
    /// Also updates the `state_mask` accordingly.
    pub const fn branch_set_child(&mut self, nibble: u8, child: Option<NodeId>) {
        if let Self::Branch { children, state_mask, hash, .. } = self {
            children[nibble as usize] = child;
            if child.is_some() {
                state_mask.set_bit(nibble);
            } else {
                state_mask.unset_bit(nibble);
            }
            *hash = None;
        }
    }
}

/// Arena-based sparse subtrie.
///
/// This replaces the `HashMap<Nibbles, SparseNode>` approach with arena allocation.
/// Nodes are accessed by traversing from the root using child references, not by path lookup.
#[derive(Clone, Debug)]
pub struct ArenaSparseSubtrie {
    /// The root path of this subtrie.
    pub path: Nibbles,
    /// Arena storing all nodes in this subtrie.
    pub arena: Arena<ArenaSparseNode>,
    /// The arena ID of the root node.
    pub root: NodeId,
    /// Map from full leaf paths to their RLP-encoded values.
    pub values: HashMap<Nibbles, Vec<u8>>,
}

impl Default for ArenaSparseSubtrie {
    fn default() -> Self {
        let mut arena = Arena::new();
        let root = arena.insert(ArenaSparseNode::Empty);
        Self { path: Nibbles::default(), arena, root, values: HashMap::default() }
    }
}

impl ArenaSparseSubtrie {
    /// Creates a new subtrie with the specified root path.
    pub fn new(path: Nibbles) -> Self {
        let mut arena = Arena::new();
        let root = arena.insert(ArenaSparseNode::Empty);
        Self { path, arena, root, values: HashMap::default() }
    }

    /// Returns true if this subtrie has no nodes (only an empty root).
    pub fn is_empty(&self) -> bool {
        self.arena.len() == 1 && matches!(self.arena.get(self.root), Some(ArenaSparseNode::Empty))
    }

    /// Clears the subtrie, resetting it to an empty state.
    pub fn clear(&mut self) {
        self.arena.clear();
        self.root = self.arena.insert(ArenaSparseNode::Empty);
        self.values.clear();
        self.path = Nibbles::default();
    }

    /// Returns a heuristic for the in-memory size of this subtrie in bytes.
    pub fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();
        size += self.arena.capacity() * core::mem::size_of::<ArenaSparseNode>();
        size += self.values.capacity() * (core::mem::size_of::<Nibbles>() + 32);
        size
    }

    /// Traverses the trie from the root to find the node at the given path.
    /// Returns `None` if the path doesn't exist or leads to a blinded node.
    pub fn find_node(&self, target_path: &Nibbles) -> Option<(NodeId, &ArenaSparseNode)> {
        if target_path.len() < self.path.len() {
            return None;
        }

        let mut current_id = self.root;
        let mut current_path = self.path;

        loop {
            let node = self.arena.get(current_id)?;

            if current_path == *target_path {
                return Some((current_id, node));
            }

            match node {
                ArenaSparseNode::Branch { children, state_mask, .. } => {
                    let nibble = target_path.get_unchecked(current_path.len());
                    if !state_mask.is_bit_set(nibble) {
                        return None;
                    }
                    current_id = children[nibble as usize]?;
                    current_path.push_unchecked(nibble);
                }
                ArenaSparseNode::Extension { key, child, .. } => {
                    if !target_path.slice(current_path.len()..).starts_with(key) {
                        return None;
                    }
                    current_id = *child;
                    current_path.extend(key);
                }
                ArenaSparseNode::Leaf { .. } |
                ArenaSparseNode::Hash(_) |
                ArenaSparseNode::Empty => {
                    return None;
                }
            }
        }
    }

    /// Traverses the trie to find the node at the given path, returning a mutable reference.
    pub fn find_node_mut(
        &mut self,
        target_path: &Nibbles,
    ) -> Option<(NodeId, &mut ArenaSparseNode)> {
        if target_path.len() < self.path.len() {
            return None;
        }

        let mut current_id = self.root;
        let mut current_path = self.path;

        loop {
            let node = self.arena.get(current_id)?;

            if current_path == *target_path {
                let _ = node;
                return Some((current_id, self.arena.get_mut(current_id)?));
            }

            match node {
                ArenaSparseNode::Branch { children, state_mask, .. } => {
                    let nibble = target_path.get_unchecked(current_path.len());
                    if !state_mask.is_bit_set(nibble) {
                        return None;
                    }
                    current_id = children[nibble as usize]?;
                    current_path.push_unchecked(nibble);
                }
                ArenaSparseNode::Extension { key, child, .. } => {
                    if !target_path.slice(current_path.len()..).starts_with(key) {
                        return None;
                    }
                    current_id = *child;
                    current_path.extend(key);
                }
                ArenaSparseNode::Leaf { .. } |
                ArenaSparseNode::Hash(_) |
                ArenaSparseNode::Empty => {
                    return None;
                }
            }
        }
    }

    /// Finds the existing node at the given path, or returns info about where to insert.
    /// Returns `(existing_id, existing_node)` if found, or `None` if it's a new node.
    fn find_existing_node(&self, path: &Nibbles) -> Option<NodeId> {
        if *path == self.path {
            return Some(self.root);
        }

        let mut current_id = self.root;
        let mut current_path = self.path;

        loop {
            let node = self.arena.get(current_id)?;

            match node {
                ArenaSparseNode::Branch { children, state_mask, .. } => {
                    let nibble = path.get_unchecked(current_path.len());
                    if !state_mask.is_bit_set(nibble) {
                        return None;
                    }
                    let child_id = children[nibble as usize]?;
                    current_path.push_unchecked(nibble);
                    if current_path == *path {
                        return Some(child_id);
                    }
                    current_id = child_id;
                }
                ArenaSparseNode::Extension { key, child, .. } => {
                    if !path.slice(current_path.len()..).starts_with(key) {
                        return None;
                    }
                    current_path.extend(key);
                    if current_path == *path {
                        return Some(*child);
                    }
                    current_id = *child;
                }
                _ => return None,
            }
        }
    }

    /// Reveals a trie node at the given path.
    ///
    /// This inserts or updates a node in the arena. If a placeholder (Hash/Empty) exists
    /// at this path, it will be replaced with the revealed node.
    #[allow(clippy::collapsible_if)]
    pub fn reveal_node(
        &mut self,
        path: Nibbles,
        node: &TrieNode,
        masks: Option<BranchNodeMasks>,
    ) -> SparseTrieResult<()> {
        let existing_id = self.find_existing_node(&path);

        if let Some(id) = existing_id {
            if let Some(existing) = self.arena.get(id) {
                let dominated = matches!(
                    (existing, node),
                    (ArenaSparseNode::Leaf { .. }, TrieNode::Leaf(_)) |
                        (ArenaSparseNode::Branch { .. }, TrieNode::Branch(_)) |
                        (ArenaSparseNode::Extension { .. }, TrieNode::Extension(_))
                );
                if dominated {
                    return Ok(());
                }
            }
        }

        match node {
            TrieNode::EmptyRoot => {
                debug_assert!(path.is_empty() || path == self.path);
                if let Some(id) = existing_id {
                    if let Some(slot) = self.arena.get_mut(id) {
                        *slot = ArenaSparseNode::Empty;
                    }
                }
            }
            TrieNode::Branch(branch) => {
                let previous_hash = existing_id.and_then(|id| {
                    self.arena.get(id).and_then(|n| {
                        if let ArenaSparseNode::Hash(h) = n {
                            Some(*h)
                        } else {
                            None
                        }
                    })
                });

                let store_in_db =
                    masks.is_some_and(|m| !m.hash_mask.is_empty() || !m.tree_mask.is_empty());

                let mut children = [None; 16];
                let mut stack_ptr = branch.as_ref().first_child_index();
                for idx in branch.state_mask.iter() {
                    let mut child_path = path;
                    child_path.push_unchecked(idx);

                    if Self::is_child_same_level(&path, &child_path) {
                        let child_id =
                            self.insert_child_node(child_path, &branch.stack[stack_ptr])?;
                        children[idx as usize] = Some(child_id);
                    }
                    stack_ptr += 1;
                }

                let new_node = ArenaSparseNode::Branch {
                    state_mask: branch.state_mask,
                    children,
                    hash: previous_hash,
                    store_in_db_trie: Some(store_in_db),
                };

                if let Some(id) = existing_id {
                    if let Some(slot) = self.arena.get_mut(id) {
                        *slot = new_node;
                    }
                }
            }
            TrieNode::Extension(ext) => {
                let previous_hash = existing_id.and_then(|id| {
                    self.arena.get(id).and_then(|n| {
                        if let ArenaSparseNode::Hash(h) = n {
                            Some(*h)
                        } else {
                            None
                        }
                    })
                });

                let mut child_path = path;
                child_path.extend(&ext.key);

                let child_id = if Self::is_child_same_level(&path, &child_path) {
                    self.insert_child_node(child_path, &ext.child)?
                } else {
                    Index::from_raw_parts(usize::MAX, u64::MAX)
                };

                let new_node = ArenaSparseNode::Extension {
                    key: ext.key,
                    child: child_id,
                    hash: previous_hash,
                    store_in_db_trie: None,
                };

                if let Some(id) = existing_id {
                    if let Some(slot) = self.arena.get_mut(id) {
                        *slot = new_node;
                    }
                }
            }
            TrieNode::Leaf(leaf) => {
                let previous_hash = existing_id.and_then(|id| {
                    self.arena.get(id).and_then(|n| {
                        if let ArenaSparseNode::Hash(h) = n {
                            Some(*h)
                        } else {
                            None
                        }
                    })
                });

                let mut full_path = path;
                full_path.extend(&leaf.key);
                self.values.insert(full_path, leaf.value.clone());

                let new_node = ArenaSparseNode::Leaf { key: leaf.key, hash: previous_hash };

                if let Some(id) = existing_id {
                    if let Some(slot) = self.arena.get_mut(id) {
                        *slot = new_node;
                    }
                }
            }
        }

        Ok(())
    }

    /// Inserts a child node from RLP-encoded data, returning its arena ID.
    /// This is used when revealing branch/extension nodes to create placeholder children.
    /// The `child_path` is used to compute full leaf paths for value storage.
    fn insert_child_node(&mut self, child_path: Nibbles, child: &[u8]) -> SparseTrieResult<NodeId> {
        if child.len() == B256::len_bytes() + 1 {
            let hash = B256::from_slice(&child[1..]);
            let id = self.arena.insert(ArenaSparseNode::Hash(hash));
            return Ok(id);
        }

        let decoded = TrieNode::decode(&mut &child[..])?;

        if let TrieNode::Leaf(ref leaf) = decoded {
            let mut full_path = child_path;
            full_path.extend(&leaf.key);
            self.values.insert(full_path, leaf.value.clone());
        }

        let id = self.arena.insert(ArenaSparseNode::from_node(decoded));
        Ok(id)
    }

    /// Reveals a child node from RLP-encoded data as the root of this subtrie.
    /// This is used when the parent (in upper subtrie) has a child that belongs in a lower subtrie.
    pub fn reveal_child_as_root(&mut self, child: &[u8]) -> SparseTrieResult<()> {
        let new_node = if child.len() == B256::len_bytes() + 1 {
            let hash = B256::from_slice(&child[1..]);
            ArenaSparseNode::Hash(hash)
        } else {
            let decoded = TrieNode::decode(&mut &child[..])?;
            ArenaSparseNode::from_node(decoded)
        };

        if let Some(slot) = self.arena.get_mut(self.root) {
            *slot = new_node;
        }
        Ok(())
    }

    /// Returns true if the current path and its child are both found in the same level.
    fn is_child_same_level(current_path: &Nibbles, child_path: &Nibbles) -> bool {
        let current_level = core::mem::discriminant(&SparseSubtrieType::from_path(current_path));
        let child_level = core::mem::discriminant(&SparseSubtrieType::from_path(child_path));
        current_level == child_level
    }
}

/// Tracks the state of lower subtries in the arena-based implementation.
#[derive(Clone, Debug)]
pub enum LowerArenaSparseSubtrie {
    /// Blinded subtrie with optional pre-allocated storage for reuse.
    Blind(Option<Box<ArenaSparseSubtrie>>),
    /// Revealed subtrie with actual nodes.
    Revealed(Box<ArenaSparseSubtrie>),
}

impl Default for LowerArenaSparseSubtrie {
    fn default() -> Self {
        Self::Blind(None)
    }
}

impl LowerArenaSparseSubtrie {
    /// Returns a reference to the underlying subtrie if revealed.
    pub fn as_revealed_ref(&self) -> Option<&ArenaSparseSubtrie> {
        match self {
            Self::Blind(_) => None,
            Self::Revealed(subtrie) => Some(subtrie.as_ref()),
        }
    }

    /// Returns a mutable reference to the underlying subtrie if revealed.
    pub fn as_revealed_mut(&mut self) -> Option<&mut ArenaSparseSubtrie> {
        match self {
            Self::Blind(_) => None,
            Self::Revealed(subtrie) => Some(subtrie.as_mut()),
        }
    }

    /// Reveals the subtrie, transitioning from Blind to Revealed state.
    pub fn reveal(&mut self, path: &Nibbles) {
        match self {
            Self::Blind(allocated) => {
                *self = if let Some(mut subtrie) = allocated.take() {
                    subtrie.path = *path;
                    Self::Revealed(subtrie)
                } else {
                    Self::Revealed(Box::new(ArenaSparseSubtrie::new(*path)))
                }
            }
            Self::Revealed(subtrie) => {
                if path.len() < subtrie.path.len() {
                    subtrie.path = *path;
                }
            }
        }
    }

    /// Clears the subtrie and transitions to blinded state.
    pub fn clear(&mut self) {
        *self = match core::mem::take(self) {
            Self::Blind(allocated) => Self::Blind(allocated),
            Self::Revealed(mut subtrie) => {
                subtrie.clear();
                Self::Blind(Some(subtrie))
            }
        }
    }
}

/// A revealed sparse trie using arena allocation for nodes.
///
/// This is structurally similar to [`crate::ParallelSparseTrie`] but uses arena-based
/// node storage instead of `HashMap`s for potentially faster traversal.
#[derive(Clone, Debug)]
pub struct ArenaParallelSparseTrie {
    /// Upper subtrie containing nodes with paths shorter than [`UPPER_TRIE_MAX_DEPTH`].
    upper_subtrie: Box<ArenaSparseSubtrie>,
    /// Array of lower subtries indexed by the first [`UPPER_TRIE_MAX_DEPTH`] nibbles.
    lower_subtries: Box<[LowerArenaSparseSubtrie; NUM_LOWER_SUBTRIES]>,
    /// Set of prefixes that have been marked as updated.
    prefix_set: PrefixSetMut,
    /// Optional tracking of trie updates.
    updates: Option<SparseTrieUpdates>,
    /// Branch node masks for database storage decisions.
    branch_node_masks: BranchNodeMasksMap,
    /// Thresholds controlling when parallelism is enabled.
    parallelism_thresholds: ParallelismThresholds,
}

impl Default for ArenaParallelSparseTrie {
    fn default() -> Self {
        Self {
            upper_subtrie: Box::new(ArenaSparseSubtrie::default()),
            lower_subtries: Box::new(
                [const { LowerArenaSparseSubtrie::Blind(None) }; NUM_LOWER_SUBTRIES],
            ),
            prefix_set: PrefixSetMut::default(),
            updates: None,
            branch_node_masks: BranchNodeMasksMap::default(),
            parallelism_thresholds: Default::default(),
        }
    }
}

impl SparseTrie for ArenaParallelSparseTrie {
    fn set_root(
        &mut self,
        root: TrieNode,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<()> {
        self.set_updates(retain_updates);
        self.reveal_upper_node(Nibbles::default(), &root, masks)
    }

    fn set_updates(&mut self, retain_updates: bool) {
        self.updates = retain_updates.then(Default::default);
    }

    fn reveal_nodes(&mut self, mut nodes: Vec<ProofTrieNode>) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        nodes.sort_unstable_by(
            |ProofTrieNode { path: path_a, .. }, ProofTrieNode { path: path_b, .. }| {
                let subtrie_type_a = SparseSubtrieType::from_path(path_a);
                let subtrie_type_b = SparseSubtrieType::from_path(path_b);
                subtrie_type_a.cmp(&subtrie_type_b).then_with(|| path_a.cmp(path_b))
            },
        );

        self.branch_node_masks.reserve(nodes.len());
        for ProofTrieNode { path, masks, .. } in &nodes {
            if let Some(branch_masks) = masks {
                self.branch_node_masks.insert(*path, *branch_masks);
            }
        }

        let num_upper_nodes = nodes
            .iter()
            .position(|n| !SparseSubtrieType::path_len_is_upper(n.path.len()))
            .unwrap_or(nodes.len());
        let (upper_nodes, lower_nodes) = nodes.split_at(num_upper_nodes);

        for node in upper_nodes {
            self.reveal_upper_node(node.path, &node.node, node.masks)?;
        }

        for node in lower_nodes {
            let subtrie_idx = path_subtrie_index(&node.path);
            self.lower_subtries[subtrie_idx].reveal(&node.path);
            if let Some(subtrie) = self.lower_subtries[subtrie_idx].as_revealed_mut() {
                subtrie.reveal_node(node.path, &node.node, node.masks)?;
            }
        }

        Ok(())
    }

    fn update_leaf<P: reth_trie_sparse::provider::TrieNodeProvider>(
        &mut self,
        _full_path: Nibbles,
        _value: Vec<u8>,
        _provider: P,
    ) -> SparseTrieResult<()> {
        Err(SparseTrieError::from(SparseTrieErrorKind::BlindedNode {
            path: Nibbles::default(),
            hash: B256::ZERO,
        }))
    }

    fn remove_leaf<P: reth_trie_sparse::provider::TrieNodeProvider>(
        &mut self,
        _full_path: &Nibbles,
        _provider: P,
    ) -> SparseTrieResult<()> {
        Err(SparseTrieError::from(SparseTrieErrorKind::BlindedNode {
            path: Nibbles::default(),
            hash: B256::ZERO,
        }))
    }

    fn root(&mut self) -> B256 {
        match self.upper_subtrie.arena.get(self.upper_subtrie.root) {
            Some(ArenaSparseNode::Empty) | None => EMPTY_ROOT_HASH,
            Some(node) => node.hash().unwrap_or(EMPTY_ROOT_HASH),
        }
    }

    fn update_subtrie_hashes(&mut self) {
        // Stub: not implemented
    }

    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>> {
        if let Some(value) = self.upper_subtrie.values.get(full_path) {
            return Some(value);
        }

        let subtrie_idx = path_subtrie_index(full_path);
        self.lower_subtries
            .get(subtrie_idx)
            .and_then(|s| s.as_revealed_ref())
            .and_then(|subtrie| subtrie.values.get(full_path))
    }

    fn find_leaf(
        &self,
        path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        if let Some(value) = self.upper_subtrie.values.get(path) {
            if let Some(expected) = expected_value {
                if value == expected {
                    return Ok(LeafLookup::Exists);
                }
                return Err(LeafLookupError::ValueMismatch {
                    path: *path,
                    expected: Some(expected.clone()),
                    actual: value.clone(),
                });
            }
            return Ok(LeafLookup::Exists);
        }

        let subtrie_idx = path_subtrie_index(path);
        // Using nested if-let instead of let chains for rustfmt compatibility
        #[allow(clippy::collapsible_if)]
        if let Some(subtrie) =
            self.lower_subtries.get(subtrie_idx).and_then(|s| s.as_revealed_ref())
        {
            if let Some(value) = subtrie.values.get(path) {
                if let Some(expected) = expected_value {
                    if value == expected {
                        return Ok(LeafLookup::Exists);
                    }
                    return Err(LeafLookupError::ValueMismatch {
                        path: *path,
                        expected: Some(expected.clone()),
                        actual: value.clone(),
                    });
                }
                return Ok(LeafLookup::Exists);
            }
        }

        Ok(LeafLookup::NonExistent)
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        match &self.updates {
            Some(updates) => Cow::Borrowed(updates),
            None => Cow::Owned(SparseTrieUpdates::default()),
        }
    }

    fn wipe(&mut self) {
        *self = Self::default();
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        self.updates.take().unwrap_or_default()
    }

    fn clear(&mut self) {
        self.upper_subtrie.clear();
        for lower in self.lower_subtries.iter_mut() {
            lower.clear();
        }
        self.prefix_set = PrefixSetMut::default();
        self.branch_node_masks.clear();
        if let Some(updates) = &mut self.updates {
            updates.updated_nodes.clear();
            updates.removed_nodes.clear();
            updates.wiped = false;
        }
    }

    fn shrink_nodes_to(&mut self, _size: usize) {
        // Arena doesn't support shrinking in the same way as HashMap
    }

    fn shrink_values_to(&mut self, size: usize) {
        self.upper_subtrie.values.shrink_to(size);
        for lower in self.lower_subtries.iter_mut() {
            if let Some(subtrie) = lower.as_revealed_mut() {
                subtrie.values.shrink_to(size);
            }
        }
    }
}

impl SparseTrieExt for ArenaParallelSparseTrie {
    fn size_hint(&self) -> usize {
        let mut count = self.upper_subtrie.arena.len();
        for lower in self.lower_subtries.iter() {
            if let Some(subtrie) = lower.as_revealed_ref() {
                count += subtrie.arena.len();
            }
        }
        count
    }

    fn prune(&mut self, _max_depth: usize) -> usize {
        // Stub: not implemented
        0
    }

    fn update_leaves(
        &mut self,
        _updates: &mut alloy_primitives::map::B256Map<LeafUpdate>,
        _proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()> {
        Err(SparseTrieError::from(SparseTrieErrorKind::BlindedNode {
            path: Nibbles::default(),
            hash: B256::ZERO,
        }))
    }
}

impl ArenaParallelSparseTrie {
    /// Sets the thresholds that control when parallelism is used during operations.
    pub const fn with_parallelism_thresholds(mut self, thresholds: ParallelismThresholds) -> Self {
        self.parallelism_thresholds = thresholds;
        self
    }

    /// Returns true if retaining updates is enabled.
    #[allow(dead_code)]
    const fn updates_enabled(&self) -> bool {
        self.updates.is_some()
    }

    /// Returns the number of nodes across all subtries.
    pub fn node_count(&self) -> usize {
        let mut count = self.upper_subtrie.arena.len();
        for lower in self.lower_subtries.iter() {
            if let Some(subtrie) = lower.as_revealed_ref() {
                count += subtrie.arena.len();
            }
        }
        count
    }

    /// Returns the number of leaf values across all subtries.
    pub fn value_count(&self) -> usize {
        let mut count = self.upper_subtrie.values.len();
        for lower in self.lower_subtries.iter() {
            if let Some(subtrie) = lower.as_revealed_ref() {
                count += subtrie.values.len();
            }
        }
        count
    }

    /// Returns a heuristic for the in-memory size of this trie in bytes.
    pub fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();
        size += self.upper_subtrie.memory_size();
        for lower in self.lower_subtries.iter() {
            if let Some(subtrie) = lower.as_revealed_ref() {
                size += subtrie.memory_size();
            }
        }
        size
    }

    /// Returns true if the trie is empty.
    pub fn is_empty(&self) -> bool {
        self.upper_subtrie.is_empty()
    }

    /// Reveals a node in the upper subtrie and handles cross-subtrie child linking.
    fn reveal_upper_node(
        &mut self,
        path: Nibbles,
        node: &TrieNode,
        masks: Option<BranchNodeMasks>,
    ) -> SparseTrieResult<()> {
        self.upper_subtrie.reveal_node(path, node, masks)?;

        match node {
            TrieNode::Branch(branch) => {
                if !SparseSubtrieType::path_len_is_upper(path.len() + 1) {
                    let mut stack_ptr = branch.as_ref().first_child_index();
                    for idx in branch.state_mask.iter() {
                        let mut child_path = path;
                        child_path.push_unchecked(idx);
                        let subtrie_idx = path_subtrie_index(&child_path);
                        self.lower_subtries[subtrie_idx].reveal(&child_path);
                        if let Some(subtrie) = self.lower_subtries[subtrie_idx].as_revealed_mut() {
                            subtrie.reveal_child_as_root(&branch.stack[stack_ptr])?;
                        }
                        stack_ptr += 1;
                    }
                }
            }
            TrieNode::Extension(ext) => {
                let mut child_path = path;
                child_path.extend(&ext.key);
                if !SparseSubtrieType::path_len_is_upper(child_path.len()) {
                    let subtrie_idx = path_subtrie_index(&child_path);
                    self.lower_subtries[subtrie_idx].reveal(&child_path);
                    if let Some(subtrie) = self.lower_subtries[subtrie_idx].as_revealed_mut() {
                        subtrie.reveal_child_as_root(&ext.child)?;
                    }
                }
            }
            TrieNode::EmptyRoot | TrieNode::Leaf(_) => (),
        }

        Ok(())
    }
}

/// Convert first [`UPPER_TRIE_MAX_DEPTH`] nibbles of the path into a lower subtrie index.
fn path_subtrie_index(path: &Nibbles) -> usize {
    debug_assert_eq!(UPPER_TRIE_MAX_DEPTH, 2);
    if path.len() < UPPER_TRIE_MAX_DEPTH {
        return 0;
    }
    path.get_byte_unchecked(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_sparse_node_basic() {
        let empty = ArenaSparseNode::Empty;
        assert!(matches!(empty.node_type(), SparseNodeType::Empty));

        let hash = ArenaSparseNode::Hash(B256::ZERO);
        assert!(hash.is_hash());

        let leaf = ArenaSparseNode::new_leaf(Nibbles::default());
        assert!(matches!(leaf.node_type(), SparseNodeType::Leaf));

        let branch = ArenaSparseNode::new_branch(TrieMask::new(0b11));
        assert!(matches!(branch.node_type(), SparseNodeType::Branch { .. }));
    }

    #[test]
    fn test_arena_subtrie_default() {
        let subtrie = ArenaSparseSubtrie::default();
        assert!(subtrie.is_empty());
        assert_eq!(subtrie.arena.len(), 1);
    }

    #[test]
    fn test_arena_parallel_sparse_trie_default() {
        let trie = ArenaParallelSparseTrie::default();
        assert!(trie.is_empty());
        assert_eq!(trie.node_count(), 1);
        assert_eq!(trie.value_count(), 0);
    }

    #[test]
    fn test_branch_child_operations() {
        let mut arena = Arena::new();
        let child_id = arena.insert(ArenaSparseNode::new_leaf(Nibbles::default()));

        let mut branch = ArenaSparseNode::new_branch(TrieMask::new(0));
        assert!(branch.branch_child(0).is_none());

        branch.branch_set_child(0, Some(child_id));
        assert_eq!(branch.branch_child(0), Some(child_id));

        if let ArenaSparseNode::Branch { state_mask, .. } = &branch {
            assert!(state_mask.is_bit_set(0));
        }

        branch.branch_set_child(0, None);
        assert!(branch.branch_child(0).is_none());
    }

    #[test]
    fn test_reveal_nodes_empty_root() {
        let mut trie = ArenaParallelSparseTrie::default();

        let nodes = vec![ProofTrieNode {
            path: Nibbles::default(),
            node: TrieNode::EmptyRoot,
            masks: None,
        }];

        trie.reveal_nodes(nodes).unwrap();
        assert!(trie.is_empty());
        assert_eq!(trie.node_count(), 1);
    }

    #[test]
    fn test_reveal_nodes_leaf() {
        use reth_trie_common::LeafNode;

        let mut trie = ArenaParallelSparseTrie::default();

        let leaf_key = Nibbles::from_nibbles([1, 2, 3, 4]);
        let leaf_value = vec![1, 2, 3, 4];

        let nodes = vec![ProofTrieNode {
            path: Nibbles::default(),
            node: TrieNode::Leaf(LeafNode::new(leaf_key, leaf_value.clone())),
            masks: None,
        }];

        trie.reveal_nodes(nodes).unwrap();

        assert!(!trie.is_empty());
        assert_eq!(trie.node_count(), 1);
        assert_eq!(trie.value_count(), 1);

        let full_path = leaf_key;
        assert_eq!(trie.get_leaf_value(&full_path), Some(&leaf_value));
    }

    #[test]
    fn test_reveal_nodes_branch() {
        use alloy_rlp::Encodable;
        use reth_trie_common::{BranchNode, LeafNode, RlpNode};

        let mut trie = ArenaParallelSparseTrie::default();

        let leaf0_key = Nibbles::from_nibbles([1, 2]);
        let leaf0_value = vec![10, 20];
        let leaf1_key = Nibbles::from_nibbles([3, 4]);
        let leaf1_value = vec![30, 40];

        let leaf0 = TrieNode::Leaf(LeafNode::new(leaf0_key, leaf0_value.clone()));
        let leaf1 = TrieNode::Leaf(LeafNode::new(leaf1_key, leaf1_value.clone()));

        let mut leaf0_rlp = Vec::new();
        leaf0.encode(&mut leaf0_rlp);
        let mut leaf1_rlp = Vec::new();
        leaf1.encode(&mut leaf1_rlp);

        let state_mask = TrieMask::new((1 << 0) | (1 << 1));
        let stack =
            vec![RlpNode::from_raw(&leaf0_rlp).unwrap(), RlpNode::from_raw(&leaf1_rlp).unwrap()];

        let branch = BranchNode::new(stack, state_mask);
        let nodes = vec![ProofTrieNode {
            path: Nibbles::default(),
            node: TrieNode::Branch(branch),
            masks: None,
        }];

        trie.reveal_nodes(nodes).unwrap();

        assert!(!trie.is_empty());
        assert_eq!(trie.node_count(), 3);
        assert_eq!(trie.value_count(), 2);

        let mut full_path0 = Nibbles::from_nibbles([0]);
        full_path0.extend(&leaf0_key);
        let mut full_path1 = Nibbles::from_nibbles([1]);
        full_path1.extend(&leaf1_key);

        assert_eq!(trie.get_leaf_value(&full_path0), Some(&leaf0_value));
        assert_eq!(trie.get_leaf_value(&full_path1), Some(&leaf1_value));
    }

    // Helper functions adapted from ParallelSparseTrie tests
    fn encode_account_value(nonce: u64) -> Vec<u8> {
        use alloy_rlp::Encodable;
        use alloy_trie::EMPTY_ROOT_HASH;
        use reth_primitives_traits::Account;

        let account = Account { nonce, ..Default::default() };
        let trie_account = account.into_trie_account(EMPTY_ROOT_HASH);
        let mut buf = Vec::new();
        trie_account.encode(&mut buf);
        buf
    }

    fn create_leaf_node(key: impl AsRef<[u8]>, value_nonce: u64) -> TrieNode {
        use reth_trie_common::LeafNode;
        TrieNode::Leaf(LeafNode::new(Nibbles::from_nibbles(key), encode_account_value(value_nonce)))
    }

    fn create_extension_node(key: impl AsRef<[u8]>, child_hash: B256) -> TrieNode {
        use reth_trie_common::{ExtensionNode, RlpNode};
        TrieNode::Extension(ExtensionNode::new(
            Nibbles::from_nibbles(key),
            RlpNode::word_rlp(&child_hash),
        ))
    }

    fn create_branch_node_with_children(
        children_indices: &[u8],
        child_hashes: impl IntoIterator<Item = reth_trie_common::RlpNode>,
    ) -> TrieNode {
        use reth_trie_common::BranchNode;

        let mut stack = Vec::new();
        let mut state_mask = TrieMask::default();

        for (&idx, hash) in children_indices.iter().zip(child_hashes.into_iter()) {
            state_mask.set_bit(idx);
            stack.push(hash);
        }

        TrieNode::Branch(BranchNode::new(stack, state_mask))
    }

    #[test]
    fn test_reveal_node_leaves() {
        use reth_trie_sparse::SparseTrie;

        // Test revealing a leaf as the root of the upper subtrie
        {
            let trie = ArenaParallelSparseTrie::default()
                .with_root(create_leaf_node([0x1, 0x2, 0x3], 42), None, true)
                .unwrap();

            let root_node =
                trie.upper_subtrie.arena.get(trie.upper_subtrie.root).expect("root in arena");
            assert!(matches!(
                root_node,
                ArenaSparseNode::Leaf { key, hash: None }
                if *key == Nibbles::from_nibbles([0x1, 0x2, 0x3])
            ));

            let full_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
            assert_eq!(trie.upper_subtrie.values.get(&full_path), Some(&encode_account_value(42)));
        }

        // Reveal leaf in a lower trie (as its root)
        {
            let mut trie = ArenaParallelSparseTrie::default();
            let path = Nibbles::from_nibbles([0x1, 0x2]);
            let node = create_leaf_node([0x3, 0x4], 42);
            let masks = None;

            trie.reveal_nodes(vec![ProofTrieNode { path, node, masks }]).unwrap();

            // Check that the lower subtrie was created
            let idx = path_subtrie_index(&path);
            assert!(trie.lower_subtries[idx].as_revealed_ref().is_some());

            // Check that the lower subtrie's path was correctly set
            let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
            assert_eq!(lower_subtrie.path, path);

            // The root of the lower subtrie is the leaf
            let root_node = lower_subtrie.arena.get(lower_subtrie.root).expect("root in arena");
            assert!(matches!(
                root_node,
                ArenaSparseNode::Leaf { key, hash: None }
                if *key == Nibbles::from_nibbles([0x3, 0x4])
            ));
        }
    }

    #[test]
    fn test_reveal_node_extension_all_upper() {
        use reth_trie_sparse::SparseTrie;

        let child_hash = B256::repeat_byte(0xab);
        let node = create_extension_node([0x1], child_hash);
        let masks = None;
        let trie = ArenaParallelSparseTrie::default().with_root(node, masks, true).unwrap();

        let root_node =
            trie.upper_subtrie.arena.get(trie.upper_subtrie.root).expect("root in arena");
        assert!(matches!(
            root_node,
            ArenaSparseNode::Extension { key, hash: None, .. }
            if *key == Nibbles::from_nibbles([0x1])
        ));

        // Child should be accessible via the extension's child pointer
        if let ArenaSparseNode::Extension { child, .. } = root_node {
            let child_node = trie.upper_subtrie.arena.get(*child).expect("child in arena");
            assert!(matches!(child_node, ArenaSparseNode::Hash(h) if *h == child_hash));
        } else {
            panic!("expected extension node");
        }
    }

    #[test]
    fn test_reveal_node_extension_cross_level() {
        use reth_trie_sparse::SparseTrie;

        let child_hash = B256::repeat_byte(0xcd);
        let node = create_extension_node([0x1, 0x2, 0x3], child_hash);
        let masks = None;
        let trie = ArenaParallelSparseTrie::default().with_root(node, masks, true).unwrap();

        // Extension node should be in upper trie
        let root_node =
            trie.upper_subtrie.arena.get(trie.upper_subtrie.root).expect("root in arena");
        assert!(matches!(
            root_node,
            ArenaSparseNode::Extension { key, hash: None, .. }
            if *key == Nibbles::from_nibbles([0x1, 0x2, 0x3])
        ));

        // Child path (0x1, 0x2, 0x3) should be in lower trie
        let child_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let idx = path_subtrie_index(&child_path);
        assert!(trie.lower_subtries[idx].as_revealed_ref().is_some());

        let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
        assert_eq!(lower_subtrie.path, child_path);

        // The lower subtrie root is the hash node
        let root_node = lower_subtrie.arena.get(lower_subtrie.root).expect("root in arena");
        assert!(matches!(root_node, ArenaSparseNode::Hash(h) if *h == child_hash));
    }

    #[test]
    fn test_reveal_node_extension_cross_level_boundary() {
        use reth_trie_sparse::SparseTrie;

        // Create an extension at root that crosses into lower trie
        // Extension key [0x1, 0x2] means child is at path [0x1, 0x2] which is in lower trie
        let ext_child_hash = B256::repeat_byte(0xcd);
        let ext_node = create_extension_node([0x1, 0x2], ext_child_hash);

        let trie = ArenaParallelSparseTrie::default().with_root(ext_node, None, true).unwrap();

        // Extension node should be at root
        let root_node =
            trie.upper_subtrie.arena.get(trie.upper_subtrie.root).expect("root in arena");
        assert!(matches!(
            root_node,
            ArenaSparseNode::Extension { key, hash: None, .. }
            if *key == Nibbles::from_nibbles([0x1, 0x2])
        ));

        // Extension's child [0x1, 0x2] should be in lower trie (path length >= 2)
        let child_path = Nibbles::from_nibbles([0x1, 0x2]);
        let idx = path_subtrie_index(&child_path);
        assert!(trie.lower_subtries[idx].as_revealed_ref().is_some());

        let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
        assert_eq!(lower_subtrie.path, child_path);

        let root_node = lower_subtrie.arena.get(lower_subtrie.root).expect("root in arena");
        assert!(matches!(root_node, ArenaSparseNode::Hash(h) if *h == ext_child_hash));
    }

    #[test]
    fn test_reveal_node_branch_all_upper() {
        use reth_trie_common::RlpNode;
        use reth_trie_sparse::SparseTrie;

        let child_hashes = [
            RlpNode::word_rlp(&B256::repeat_byte(0x11)),
            RlpNode::word_rlp(&B256::repeat_byte(0x22)),
        ];
        let node = create_branch_node_with_children(&[0x0, 0x5], child_hashes.clone());
        let masks = None;
        let trie = ArenaParallelSparseTrie::default().with_root(node, masks, true).unwrap();

        // Branch node should be in upper trie
        let root_node =
            trie.upper_subtrie.arena.get(trie.upper_subtrie.root).expect("root in arena");
        assert!(matches!(
            root_node,
            ArenaSparseNode::Branch { state_mask, hash: None, .. }
            if *state_mask == TrieMask::new(0b0000000000100001)
        ));

        // Children should be accessible via the branch's children array
        if let ArenaSparseNode::Branch { children, .. } = root_node {
            let child0_id = children[0x0].expect("child 0 should exist");
            let child0_node = trie.upper_subtrie.arena.get(child0_id).expect("child 0 in arena");
            assert!(
                matches!(child0_node, ArenaSparseNode::Hash(h) if *h == child_hashes[0].as_hash().unwrap())
            );

            let child5_id = children[0x5].expect("child 5 should exist");
            let child5_node = trie.upper_subtrie.arena.get(child5_id).expect("child 5 in arena");
            assert!(
                matches!(child5_node, ArenaSparseNode::Hash(h) if *h == child_hashes[1].as_hash().unwrap())
            );
        } else {
            panic!("expected branch node");
        }
    }

    #[test]
    fn test_reveal_node_branch_cross_level() {
        use reth_trie_common::RlpNode;
        use reth_trie_sparse::SparseTrie;

        // Create a branch at root with children at paths [0x0], [0x7], [0xf]
        // These are at depth 1 which is still upper trie, but when the branch is at depth 1,
        // children at depth 2 go to lower tries
        let child_hashes = [
            RlpNode::word_rlp(&B256::repeat_byte(0x33)),
            RlpNode::word_rlp(&B256::repeat_byte(0x44)),
            RlpNode::word_rlp(&B256::repeat_byte(0x55)),
        ];
        // Create a branch at root with children at [0x1, 0x0], [0x1, 0x7], [0x1, 0xf]
        // by first creating an extension from root to the branch
        let branch_node = create_branch_node_with_children(&[0x0, 0x7, 0xf], child_hashes.clone());

        // Create extension at root pointing to the branch
        let ext_hash = B256::repeat_byte(0xee);
        let ext_node = create_extension_node([0x1], ext_hash);

        // Set extension as root, then reveal the branch
        let mut trie = ArenaParallelSparseTrie::default().with_root(ext_node, None, true).unwrap();

        // Now reveal the branch at path [0x1]
        // First we need to set up the reveal properly - the extension child is a hash,
        // we need to "reveal" it as a branch
        // This simulates what happens when reveal_nodes receives both extension and branch
        let branch_path = Nibbles::from_nibbles([0x1]);
        trie.reveal_nodes(vec![ProofTrieNode {
            path: branch_path,
            node: branch_node,
            masks: None,
        }])
        .unwrap();

        // Extension node should be at root
        let root_node =
            trie.upper_subtrie.arena.get(trie.upper_subtrie.root).expect("root in arena");
        assert!(matches!(
            root_node,
            ArenaSparseNode::Extension { key, .. }
            if *key == Nibbles::from_nibbles([0x1])
        ));

        // The extension's child should now be the branch
        if let ArenaSparseNode::Extension { child, .. } = root_node {
            let branch_node = trie.upper_subtrie.arena.get(*child).expect("branch in arena");
            // The branch preserves the hash from the previous Hash node it replaced
            assert!(
                matches!(
                    branch_node,
                    ArenaSparseNode::Branch { state_mask, .. }
                    if *state_mask == TrieMask::new(0b1000000010000001)
                ),
                "expected branch, got {branch_node:?}"
            );
        } else {
            panic!("expected extension node");
        }
    }
}
