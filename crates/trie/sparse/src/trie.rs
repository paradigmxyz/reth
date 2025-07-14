use crate::{
    blinded::{BlindedProvider, RevealedNode},
    LeafLookup, LeafLookupError, SparseTrieInterface, SparseTrieUpdates, TrieMasks,
};
use alloc::{
    borrow::Cow,
    boxed::Box,
    fmt,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use alloy_primitives::{
    hex, keccak256,
    map::{Entry, HashMap, HashSet},
    B256,
};
use alloy_rlp::Decodable;
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{
    prefix_set::{PrefixSet, PrefixSetMut},
    BranchNodeCompact, BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, RlpNode, TrieMask,
    TrieNode, CHILD_INDEX_RANGE, EMPTY_ROOT_HASH,
};
use smallvec::SmallVec;
use tracing::trace;

/// The level below which the sparse trie hashes are calculated in
/// [`SerialSparseTrie::update_subtrie_hashes`].
const SPARSE_TRIE_SUBTRIE_HASHES_LEVEL: usize = 2;

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
#[derive(PartialEq, Eq, Debug)]
pub enum SparseTrie<T = SerialSparseTrie> {
    /// The trie is blind -- no nodes have been revealed
    ///
    /// This is the default state. In this state, the trie cannot be directly queried or modified
    /// until nodes are revealed.
    ///
    /// In this state the `SparseTrie` can optionally carry with it a cleared `SerialSparseTrie`.
    /// This allows for reusing the trie's allocations between payload executions.
    Blind(Option<Box<T>>),
    /// Some nodes in the Trie have been revealed.
    ///
    /// In this state, the trie can be queried and modified for the parts
    /// that have been revealed. Other parts remain blind and require revealing
    /// before they can be accessed.
    Revealed(Box<T>),
}

impl<T: Default> Default for SparseTrie<T> {
    fn default() -> Self {
        Self::Blind(None)
    }
}

impl<T: SparseTrieInterface + Default> SparseTrie<T> {
    /// Creates a new revealed but empty sparse trie with `SparseNode::Empty` as root node.
    ///
    /// # Examples
    ///
    /// ```
    /// use reth_trie_sparse::{blinded::DefaultBlindedProvider, SerialSparseTrie, SparseTrie};
    ///
    /// let trie = SparseTrie::<SerialSparseTrie>::revealed_empty();
    /// assert!(!trie.is_blind());
    /// ```
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
    /// A mutable reference to the underlying [`SparseTrieInterface`].
    pub fn reveal_root(
        &mut self,
        root: TrieNode,
        masks: TrieMasks,
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

            *revealed_trie = revealed_trie.with_root(root, masks, retain_updates)?;
            *self = Self::Revealed(revealed_trie);
        }

        Ok(self.as_revealed_mut().unwrap())
    }
}

impl<T: SparseTrieInterface> SparseTrie<T> {
    /// Creates a new blind sparse trie.
    ///
    /// # Examples
    ///
    /// ```
    /// use reth_trie_sparse::{blinded::DefaultBlindedProvider, SerialSparseTrie, SparseTrie};
    ///
    /// let trie = SparseTrie::<SerialSparseTrie>::blind();
    /// assert!(trie.is_blind());
    /// let trie = SparseTrie::<SerialSparseTrie>::default();
    /// assert!(trie.is_blind());
    /// ```
    pub const fn blind() -> Self {
        Self::Blind(None)
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

    /// Returns a [`SparseTrie::Blind`] based on this one. If this instance was revealed, or was
    /// itself a `Blind` with a pre-allocated [`SparseTrieInterface`], this will return
    /// a `Blind` carrying a cleared pre-allocated [`SparseTrieInterface`].
    pub fn clear(self) -> Self {
        match self {
            Self::Blind(_) => self,
            Self::Revealed(mut trie) => {
                trie.clear();
                Self::Blind(Some(trie))
            }
        }
    }

    /// Updates (or inserts) a leaf at the given key path with the specified RLP-encoded value.
    ///
    /// # Errors
    ///
    /// Returns an error if the trie is still blind, or if the update fails.
    pub fn update_leaf(
        &mut self,
        path: Nibbles,
        value: Vec<u8>,
        provider: impl BlindedProvider,
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
    pub fn remove_leaf(
        &mut self,
        path: &Nibbles,
        provider: impl BlindedProvider,
    ) -> SparseTrieResult<()> {
        let revealed = self.as_revealed_mut().ok_or(SparseTrieErrorKind::Blind)?;
        revealed.remove_leaf(path, provider)?;
        Ok(())
    }
}

/// The representation of revealed sparse trie.
///
/// The revealed sparse trie contains the actual trie structure with nodes, values, and
/// tracking for changes. It supports operations like inserting, updating, and removing
/// nodes.
///
///
/// ## Invariants
///
/// - The root node is always present in `nodes` collection.
/// - Each leaf entry in `nodes` collection must have a corresponding entry in `values` collection.
///   The opposite is also true.
/// - All keys in `values` collection are full leaf paths.
#[derive(Clone, PartialEq, Eq)]
pub struct SerialSparseTrie {
    /// Map from a path (nibbles) to its corresponding sparse trie node.
    /// This contains all of the revealed nodes in trie.
    nodes: HashMap<Nibbles, SparseNode>,
    /// When a branch is set, the corresponding child subtree is stored in the database.
    branch_node_tree_masks: HashMap<Nibbles, TrieMask>,
    /// When a bit is set, the corresponding child is stored as a hash in the database.
    branch_node_hash_masks: HashMap<Nibbles, TrieMask>,
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
    /// Set of prefixes (key paths) that have been marked as updated.
    /// This is used to track which parts of the trie need to be recalculated.
    prefix_set: PrefixSetMut,
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
    /// Reusable buffer for RLP encoding of nodes.
    rlp_buf: Vec<u8>,
}

impl fmt::Debug for SerialSparseTrie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialSparseTrie")
            .field("nodes", &self.nodes)
            .field("branch_tree_masks", &self.branch_node_tree_masks)
            .field("branch_hash_masks", &self.branch_node_hash_masks)
            .field("values", &self.values)
            .field("prefix_set", &self.prefix_set)
            .field("updates", &self.updates)
            .field("rlp_buf", &hex::encode(&self.rlp_buf))
            .finish_non_exhaustive()
    }
}

/// Turns a [`Nibbles`] into a [`String`] by concatenating each nibbles' hex character.
fn encode_nibbles(nibbles: &Nibbles) -> String {
    let encoded = hex::encode(nibbles.pack());
    encoded[..nibbles.len()].to_string()
}

impl fmt::Display for SerialSparseTrie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // This prints the trie in preorder traversal, using a stack
        let mut stack = Vec::new();
        let mut visited = HashSet::new();

        // 4 spaces as indent per level
        const INDENT: &str = "    ";

        // Track both path and depth
        stack.push((Nibbles::default(), self.nodes_ref().get(&Nibbles::default()).unwrap(), 0));

        while let Some((path, node, depth)) = stack.pop() {
            if !visited.insert(path) {
                continue;
            }

            // Add indentation if alternate flag (#) is set
            if f.alternate() {
                write!(f, "{}", INDENT.repeat(depth))?;
            }

            let packed_path = if depth == 0 { String::from("Root") } else { encode_nibbles(&path) };

            match node {
                SparseNode::Empty | SparseNode::Hash(_) => {
                    writeln!(f, "{packed_path} -> {node:?}")?;
                }
                SparseNode::Leaf { key, .. } => {
                    // we want to append the key to the path
                    let mut full_path = path;
                    full_path.extend(key);
                    let packed_path = encode_nibbles(&full_path);

                    writeln!(f, "{packed_path} -> {node:?}")?;
                }
                SparseNode::Extension { key, .. } => {
                    writeln!(f, "{packed_path} -> {node:?}")?;

                    // push the child node onto the stack with increased depth
                    let mut child_path = path;
                    child_path.extend(key);
                    if let Some(child_node) = self.nodes_ref().get(&child_path) {
                        stack.push((child_path, child_node, depth + 1));
                    }
                }
                SparseNode::Branch { state_mask, .. } => {
                    writeln!(f, "{packed_path} -> {node:?}")?;

                    for i in CHILD_INDEX_RANGE.rev() {
                        if state_mask.is_bit_set(i) {
                            let mut child_path = path;
                            child_path.push_unchecked(i);
                            if let Some(child_node) = self.nodes_ref().get(&child_path) {
                                stack.push((child_path, child_node, depth + 1));
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for SerialSparseTrie {
    fn default() -> Self {
        Self {
            nodes: HashMap::from_iter([(Nibbles::default(), SparseNode::Empty)]),
            branch_node_tree_masks: HashMap::default(),
            branch_node_hash_masks: HashMap::default(),
            values: HashMap::default(),
            prefix_set: PrefixSetMut::default(),
            updates: None,
            rlp_buf: Vec::new(),
        }
    }
}

impl SparseTrieInterface for SerialSparseTrie {
    fn with_root(
        mut self,
        root: TrieNode,
        masks: TrieMasks,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        self = self.with_updates(retain_updates);

        // A fresh/cleared `SerialSparseTrie` has a `SparseNode::Empty` at its root. Delete that
        // so we can reveal the new root node.
        let path = Nibbles::default();
        let _removed_root = self.nodes.remove(&path).expect("root node should exist");
        debug_assert_eq!(_removed_root, SparseNode::Empty);

        self.reveal_node(path, root, masks)?;
        Ok(self)
    }

    fn with_updates(mut self, retain_updates: bool) -> Self {
        if retain_updates {
            self.updates = Some(SparseTrieUpdates::default());
        }
        self
    }

    fn reserve_nodes(&mut self, additional: usize) {
        self.nodes.reserve(additional);
    }

    fn reveal_node(
        &mut self,
        path: Nibbles,
        node: TrieNode,
        masks: TrieMasks,
    ) -> SparseTrieResult<()> {
        // If the node is already revealed and it's not a hash node, do nothing.
        if self.nodes.get(&path).is_some_and(|node| !node.is_hash()) {
            return Ok(())
        }

        if let Some(tree_mask) = masks.tree_mask {
            self.branch_node_tree_masks.insert(path, tree_mask);
        }
        if let Some(hash_mask) = masks.hash_mask {
            self.branch_node_hash_masks.insert(path, hash_mask);
        }

        match node {
            TrieNode::EmptyRoot => {
                // For an empty root, ensure that we are at the root path.
                debug_assert!(path.is_empty());
                self.nodes.insert(path, SparseNode::Empty);
            }
            TrieNode::Branch(branch) => {
                // For a branch node, iterate over all potential children
                let mut stack_ptr = branch.as_ref().first_child_index();
                for idx in CHILD_INDEX_RANGE {
                    if branch.state_mask.is_bit_set(idx) {
                        let mut child_path = path;
                        child_path.push_unchecked(idx);
                        // Reveal each child node or hash it has
                        self.reveal_node_or_hash(child_path, &branch.stack[stack_ptr])?;
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
                                path: *entry.key(),
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
            TrieNode::Extension(ext) => match self.nodes.entry(path) {
                Entry::Occupied(mut entry) => match entry.get() {
                    // Replace a hash node with a revealed extension node.
                    SparseNode::Hash(hash) => {
                        let mut child_path = *entry.key();
                        child_path.extend(&ext.key);
                        entry.insert(SparseNode::Extension {
                            key: ext.key,
                            // Memoize the hash of a previously blinded node in a new extension
                            // node.
                            hash: Some(*hash),
                            store_in_db_trie: None,
                        });
                        self.reveal_node_or_hash(child_path, &ext.child)?;
                    }
                    // Extension node already exists, or an extension node was placed where a branch
                    // node was before.
                    SparseNode::Extension { .. } | SparseNode::Branch { .. } => {}
                    // All other node types can't be handled.
                    node @ (SparseNode::Empty | SparseNode::Leaf { .. }) => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: *entry.key(),
                            node: Box::new(node.clone()),
                        }
                        .into())
                    }
                },
                Entry::Vacant(entry) => {
                    let mut child_path = *entry.key();
                    child_path.extend(&ext.key);
                    entry.insert(SparseNode::new_ext(ext.key));
                    self.reveal_node_or_hash(child_path, &ext.child)?;
                }
            },
            TrieNode::Leaf(leaf) => match self.nodes.entry(path) {
                Entry::Occupied(mut entry) => match entry.get() {
                    // Replace a hash node with a revealed leaf node and store leaf node value.
                    SparseNode::Hash(hash) => {
                        let mut full = *entry.key();
                        full.extend(&leaf.key);
                        self.values.insert(full, leaf.value);
                        entry.insert(SparseNode::Leaf {
                            key: leaf.key,
                            // Memoize the hash of a previously blinded node in a new leaf
                            // node.
                            hash: Some(*hash),
                        });
                    }
                    // Left node already exists.
                    SparseNode::Leaf { .. } => {}
                    // All other node types can't be handled.
                    node @ (SparseNode::Empty |
                    SparseNode::Extension { .. } |
                    SparseNode::Branch { .. }) => {
                        return Err(SparseTrieErrorKind::Reveal {
                            path: *entry.key(),
                            node: Box::new(node.clone()),
                        }
                        .into())
                    }
                },
                Entry::Vacant(entry) => {
                    let mut full = *entry.key();
                    full.extend(&leaf.key);
                    entry.insert(SparseNode::new_leaf(leaf.key));
                    self.values.insert(full, leaf.value);
                }
            },
        }

        Ok(())
    }

    fn update_leaf<P: BlindedProvider>(
        &mut self,
        full_path: Nibbles,
        value: Vec<u8>,
        provider: P,
    ) -> SparseTrieResult<()> {
        self.prefix_set.insert(full_path);
        let existing = self.values.insert(full_path, value);
        if existing.is_some() {
            // trie structure unchanged, return immediately
            return Ok(())
        }

        let mut current = Nibbles::default();
        while let Some(node) = self.nodes.get_mut(&current) {
            match node {
                SparseNode::Empty => {
                    *node = SparseNode::new_leaf(full_path);
                    break
                }
                &mut SparseNode::Hash(hash) => {
                    return Err(SparseTrieErrorKind::BlindedNode { path: current, hash }.into())
                }
                SparseNode::Leaf { key: current_key, .. } => {
                    current.extend(current_key);

                    // this leaf is being updated
                    if current == full_path {
                        unreachable!("we already checked leaf presence in the beginning");
                    }

                    // find the common prefix
                    let common = current.common_prefix_length(&full_path);

                    // update existing node
                    let new_ext_key = current.slice(current.len() - current_key.len()..common);
                    *node = SparseNode::new_ext(new_ext_key);

                    // create a branch node and corresponding leaves
                    self.nodes.reserve(3);
                    self.nodes.insert(
                        current.slice(..common),
                        SparseNode::new_split_branch(
                            current.get_unchecked(common),
                            full_path.get_unchecked(common),
                        ),
                    );
                    self.nodes.insert(
                        full_path.slice(..=common),
                        SparseNode::new_leaf(full_path.slice(common + 1..)),
                    );
                    self.nodes.insert(
                        current.slice(..=common),
                        SparseNode::new_leaf(current.slice(common + 1..)),
                    );

                    break;
                }
                SparseNode::Extension { key, .. } => {
                    current.extend(key);

                    if !full_path.starts_with(&current) {
                        // find the common prefix
                        let common = current.common_prefix_length(&full_path);
                        *key = current.slice(current.len() - key.len()..common);

                        // If branch node updates retention is enabled, we need to query the
                        // extension node child to later set the hash mask for a parent branch node
                        // correctly.
                        if self.updates.is_some() {
                            // Check if the extension node child is a hash that needs to be revealed
                            if self.nodes.get(&current).unwrap().is_hash() {
                                if let Some(RevealedNode { node, tree_mask, hash_mask }) =
                                    provider.blinded_node(&current)?
                                {
                                    let decoded = TrieNode::decode(&mut &node[..])?;
                                    trace!(
                                        target: "trie::sparse",
                                        ?current,
                                        ?decoded,
                                        ?tree_mask,
                                        ?hash_mask,
                                        "Revealing extension node child",
                                    );
                                    self.reveal_node(
                                        current,
                                        decoded,
                                        TrieMasks { hash_mask, tree_mask },
                                    )?;
                                }
                            }
                        }

                        // create state mask for new branch node
                        // NOTE: this might overwrite the current extension node
                        self.nodes.reserve(3);
                        let branch = SparseNode::new_split_branch(
                            current.get_unchecked(common),
                            full_path.get_unchecked(common),
                        );
                        self.nodes.insert(current.slice(..common), branch);

                        // create new leaf
                        let new_leaf = SparseNode::new_leaf(full_path.slice(common + 1..));
                        self.nodes.insert(full_path.slice(..=common), new_leaf);

                        // recreate extension to previous child if needed
                        let key = current.slice(common + 1..);
                        if !key.is_empty() {
                            self.nodes.insert(current.slice(..=common), SparseNode::new_ext(key));
                        }

                        break;
                    }
                }
                SparseNode::Branch { state_mask, .. } => {
                    let nibble = full_path.get_unchecked(current.len());
                    current.push_unchecked(nibble);
                    if !state_mask.is_bit_set(nibble) {
                        state_mask.set_bit(nibble);
                        let new_leaf = SparseNode::new_leaf(full_path.slice(current.len()..));
                        self.nodes.insert(current, new_leaf);
                        break;
                    }
                }
            };
        }

        Ok(())
    }

    fn remove_leaf<P: BlindedProvider>(
        &mut self,
        full_path: &Nibbles,
        provider: P,
    ) -> SparseTrieResult<()> {
        if self.values.remove(full_path).is_none() {
            if let Some(&SparseNode::Hash(hash)) = self.nodes.get(full_path) {
                // Leaf is present in the trie, but it's blinded.
                return Err(SparseTrieErrorKind::BlindedNode { path: *full_path, hash }.into())
            }

            trace!(target: "trie::sparse", ?full_path, "Leaf node is not present in the trie");
            // Leaf is not present in the trie.
            return Ok(())
        }
        self.prefix_set.insert(*full_path);

        // If the path wasn't present in `values`, we still need to walk the trie and ensure that
        // there is no node at the path. When a leaf node is a blinded `Hash`, it will have an entry
        // in `nodes`, but not in the `values`.

        let mut removed_nodes = self.take_nodes_for_path(full_path)?;
        // Pop the first node from the stack which is the leaf node we want to remove.
        let mut child = removed_nodes.pop().expect("leaf exists");
        #[cfg(debug_assertions)]
        {
            let mut child_path = child.path;
            let SparseNode::Leaf { key, .. } = &child.node else { panic!("expected leaf node") };
            child_path.extend(key);
            assert_eq!(&child_path, full_path);
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
                SparseNode::Empty => return Err(SparseTrieErrorKind::Blind.into()),
                &SparseNode::Hash(hash) => {
                    return Err(SparseTrieErrorKind::BlindedNode { path: removed_path, hash }.into())
                }
                SparseNode::Leaf { .. } => {
                    unreachable!("we already popped the leaf node")
                }
                SparseNode::Extension { key, .. } => {
                    // If the node is an extension node, we need to look at its child to see if we
                    // need to merge them.
                    match &child.node {
                        SparseNode::Empty => return Err(SparseTrieErrorKind::Blind.into()),
                        &SparseNode::Hash(hash) => {
                            return Err(
                                SparseTrieErrorKind::BlindedNode { path: child.path, hash }.into()
                            )
                        }
                        // For a leaf node, we collapse the extension node into a leaf node,
                        // extending the key. While it's impossible to encounter an extension node
                        // followed by a leaf node in a complete trie, it's possible here because we
                        // could have downgraded the extension node's child into a leaf node from
                        // another node type.
                        SparseNode::Leaf { key: leaf_key, .. } => {
                            self.nodes.remove(&child.path);

                            let mut new_key = *key;
                            new_key.extend(leaf_key);
                            SparseNode::new_leaf(new_key)
                        }
                        // For an extension node, we collapse them into one extension node,
                        // extending the key
                        SparseNode::Extension { key: extension_key, .. } => {
                            self.nodes.remove(&child.path);

                            let mut new_key = *key;
                            new_key.extend(extension_key);
                            SparseNode::new_ext(new_key)
                        }
                        // For a branch node, we just leave the extension node as-is.
                        SparseNode::Branch { .. } => removed_node.node,
                    }
                }
                &SparseNode::Branch { mut state_mask, hash: _, store_in_db_trie: _ } => {
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
                        let mut child_path = removed_path;
                        child_path.push_unchecked(child_nibble);

                        trace!(target: "trie::sparse", ?removed_path, ?child_path, "Branch node has only one child");

                        if self.nodes.get(&child_path).unwrap().is_hash() {
                            trace!(target: "trie::sparse", ?child_path, "Retrieving remaining blinded branch child");
                            if let Some(RevealedNode { node, tree_mask, hash_mask }) =
                                provider.blinded_node(&child_path)?
                            {
                                let decoded = TrieNode::decode(&mut &node[..])?;
                                trace!(
                                    target: "trie::sparse",
                                    ?child_path,
                                    ?decoded,
                                    ?tree_mask,
                                    ?hash_mask,
                                    "Revealing remaining blinded branch child"
                                );
                                self.reveal_node(
                                    child_path,
                                    decoded,
                                    TrieMasks { hash_mask, tree_mask },
                                )?;
                            }
                        }

                        // Get the only child node.
                        let child = self.nodes.get(&child_path).unwrap();

                        let mut delete_child = false;
                        let new_node = match child {
                            SparseNode::Empty => return Err(SparseTrieErrorKind::Blind.into()),
                            &SparseNode::Hash(hash) => {
                                return Err(SparseTrieErrorKind::BlindedNode {
                                    path: child_path,
                                    hash,
                                }
                                .into())
                            }
                            // If the only child is a leaf node, we downgrade the branch node into a
                            // leaf node, prepending the nibble to the key, and delete the old
                            // child.
                            SparseNode::Leaf { key, .. } => {
                                delete_child = true;

                                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                                new_key.extend(key);
                                SparseNode::new_leaf(new_key)
                            }
                            // If the only child node is an extension node, we downgrade the branch
                            // node into an even longer extension node, prepending the nibble to the
                            // key, and delete the old child.
                            SparseNode::Extension { key, .. } => {
                                delete_child = true;

                                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                                new_key.extend(key);
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
                            updates.updated_nodes.remove(&removed_path);
                            updates.removed_nodes.insert(removed_path);
                        }

                        new_node
                    }
                    // If more than one child is left set in the branch, we just re-insert it as-is.
                    else {
                        SparseNode::new_branch(state_mask)
                    }
                }
            };

            child = RemovedSparseNode {
                path: removed_path,
                node: new_node.clone(),
                unset_branch_nibble: None,
            };
            trace!(target: "trie::sparse", ?removed_path, ?new_node, "Re-inserting the node");
            self.nodes.insert(removed_path, new_node);
        }

        Ok(())
    }

    fn root(&mut self) -> B256 {
        // Take the current prefix set
        let mut prefix_set = core::mem::take(&mut self.prefix_set).freeze();
        let rlp_node = self.rlp_node_allocate(&mut prefix_set);
        if let Some(root_hash) = rlp_node.as_hash() {
            root_hash
        } else {
            keccak256(rlp_node)
        }
    }

    fn update_subtrie_hashes(&mut self) {
        self.update_rlp_node_level(SPARSE_TRIE_SUBTRIE_HASHES_LEVEL);
    }

    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>> {
        self.values.get(full_path)
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.updates.as_ref().map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        self.updates.take().unwrap_or_default()
    }

    fn wipe(&mut self) {
        self.nodes = HashMap::from_iter([(Nibbles::default(), SparseNode::Empty)]);
        self.values = HashMap::default();
        self.prefix_set = PrefixSetMut::all();
        self.updates = self.updates.is_some().then(SparseTrieUpdates::wiped);
    }

    fn clear(&mut self) {
        self.nodes.clear();
        self.nodes.insert(Nibbles::default(), SparseNode::Empty);

        self.branch_node_tree_masks.clear();
        self.branch_node_hash_masks.clear();
        self.values.clear();
        self.prefix_set.clear();
        self.updates = None;
        self.rlp_buf.clear();
    }

    fn find_leaf(
        &self,
        full_path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        // Helper function to check if a value matches the expected value
        fn check_value_match(
            actual_value: &Vec<u8>,
            expected_value: Option<&Vec<u8>>,
            path: &Nibbles,
        ) -> Result<(), LeafLookupError> {
            if let Some(expected) = expected_value {
                if actual_value != expected {
                    return Err(LeafLookupError::ValueMismatch {
                        path: *path,
                        expected: Some(expected.clone()),
                        actual: actual_value.clone(),
                    });
                }
            }
            Ok(())
        }

        let mut current = Nibbles::default(); // Start at the root

        // Inclusion proof
        //
        // First, do a quick check if the value exists in our values map.
        // We assume that if there exists a leaf node, then its value will
        // be in the `values` map.
        if let Some(actual_value) = self.values.get(full_path) {
            // We found the leaf, check if the value matches (if expected value was provided)
            check_value_match(actual_value, expected_value, full_path)?;
            return Ok(LeafLookup::Exists);
        }

        // If the value does not exist in the `values` map, then this means that the leaf either:
        // - Does not exist in the trie
        // - Is missing from the witness
        // We traverse the trie to find the location where this leaf would have been, showing
        // that it is not in the trie. Or we find a blinded node, showing that the witness is
        // not complete.
        while current.len() < full_path.len() {
            match self.nodes.get(&current) {
                Some(SparseNode::Empty) | None => {
                    // None implies no node is at the current path (even in the full trie)
                    // Empty node means there is a node at this path and it is "Empty"
                    return Ok(LeafLookup::NonExistent);
                }
                Some(&SparseNode::Hash(hash)) => {
                    // We hit a blinded node - cannot determine if leaf exists
                    return Err(LeafLookupError::BlindedNode { path: current, hash });
                }
                Some(SparseNode::Leaf { key, .. }) => {
                    // We found a leaf node before reaching our target depth
                    current.extend(key);
                    if &current == full_path {
                        // This should have been handled by our initial values map check
                        if let Some(value) = self.values.get(full_path) {
                            check_value_match(value, expected_value, full_path)?;
                            return Ok(LeafLookup::Exists);
                        }
                    }

                    // The leaf node's path doesn't match our target path,
                    // providing an exclusion proof
                    return Ok(LeafLookup::NonExistent);
                }
                Some(SparseNode::Extension { key, .. }) => {
                    // Temporarily append the extension key to `current`
                    let saved_len = current.len();
                    current.extend(key);

                    if full_path.len() < current.len() || !full_path.starts_with(&current) {
                        current.truncate(saved_len); // restore
                        return Ok(LeafLookup::NonExistent);
                    }
                    // Prefix matched, so we keep walking with the longer `current`.
                }
                Some(SparseNode::Branch { state_mask, .. }) => {
                    // Check if branch has a child at the next nibble in our path
                    let nibble = full_path.get_unchecked(current.len());
                    if !state_mask.is_bit_set(nibble) {
                        // No child at this nibble - exclusion proof
                        return Ok(LeafLookup::NonExistent);
                    }

                    // Continue down the branch
                    current.push_unchecked(nibble);
                }
            }
        }

        // We've traversed to the end of the path and didn't find a leaf
        // Check if there's a node exactly at our target path
        match self.nodes.get(full_path) {
            Some(SparseNode::Leaf { key, .. }) if key.is_empty() => {
                // We found a leaf with an empty key (exact match)
                // This should be handled by the values map check above
                if let Some(value) = self.values.get(full_path) {
                    check_value_match(value, expected_value, full_path)?;
                    return Ok(LeafLookup::Exists);
                }
            }
            Some(&SparseNode::Hash(hash)) => {
                return Err(LeafLookupError::BlindedNode { path: *full_path, hash });
            }
            _ => {
                // No leaf at exactly the target path
                return Ok(LeafLookup::NonExistent);
            }
        }

        // If we get here, there's no leaf at the target path
        Ok(LeafLookup::NonExistent)
    }
}

impl SerialSparseTrie {
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
    pub fn from_root(
        root: TrieNode,
        masks: TrieMasks,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        Self::default().with_root(root, masks, retain_updates)
    }

    /// Returns a reference to the current sparse trie updates.
    ///
    /// If no updates have been made/recorded, returns an empty update set.
    pub fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.updates.as_ref().map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    /// Returns an immutable reference to all nodes in the sparse trie.
    pub const fn nodes_ref(&self) -> &HashMap<Nibbles, SparseNode> {
        &self.nodes
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
                            path: *entry.key(),
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

        self.reveal_node(path, TrieNode::decode(&mut &child[..])?, TrieMasks::none())
    }

    /// Traverse the trie from the root down to the leaf at the given path,
    /// removing and collecting all nodes along that path.
    ///
    /// This helper function is used during leaf removal to extract the nodes of the trie
    /// that will be affected by the deletion. These nodes are then re-inserted and modified
    /// as needed (collapsing extension nodes etc) given that the leaf has now been removed.
    ///
    /// # Returns
    ///
    /// Returns a vector of [`RemovedSparseNode`] representing the nodes removed during the
    /// traversal.
    ///
    /// # Errors
    ///
    /// Returns an error if a blinded node or an empty node is encountered unexpectedly,
    /// as these prevent proper removal of the leaf.
    fn take_nodes_for_path(&mut self, path: &Nibbles) -> SparseTrieResult<Vec<RemovedSparseNode>> {
        let mut current = Nibbles::default(); // Start traversal from the root
        let mut nodes = Vec::new(); // Collect traversed nodes

        while let Some(node) = self.nodes.remove(&current) {
            match &node {
                SparseNode::Empty => return Err(SparseTrieErrorKind::Blind.into()),
                &SparseNode::Hash(hash) => {
                    return Err(SparseTrieErrorKind::BlindedNode { path: current, hash }.into())
                }
                SparseNode::Leaf { key: _key, .. } => {
                    // Leaf node is always the one that we're deleting, and no other leaf nodes can
                    // be found during traversal.

                    #[cfg(debug_assertions)]
                    {
                        let mut current = current;
                        current.extend(_key);
                        assert_eq!(&current, path);
                    }

                    nodes.push(RemovedSparseNode {
                        path: current,
                        node,
                        unset_branch_nibble: None,
                    });
                    break
                }
                SparseNode::Extension { key, .. } => {
                    #[cfg(debug_assertions)]
                    {
                        let mut current = current;
                        current.extend(key);
                        assert!(
                            path.starts_with(&current),
                            "path: {path:?}, current: {current:?}, key: {key:?}",
                        );
                    }

                    let path = current;
                    current.extend(key);
                    nodes.push(RemovedSparseNode { path, node, unset_branch_nibble: None });
                }
                SparseNode::Branch { state_mask, .. } => {
                    let nibble = path.get_unchecked(current.len());
                    debug_assert!(
                        state_mask.is_bit_set(nibble),
                        "current: {current:?}, path: {path:?}, nibble: {nibble:?}, state_mask: {state_mask:?}",
                    );

                    // If the branch node has a child that is a leaf node that we're removing,
                    // we need to unset this nibble.
                    // Any other branch nodes will not require unsetting the nibble, because
                    // deleting one leaf node can not remove the whole path
                    // where the branch node is located.
                    let mut child_path = current;
                    child_path.push_unchecked(nibble);
                    let unset_branch_nibble = self
                        .nodes
                        .get(&child_path)
                        .is_some_and(move |node| match node {
                            SparseNode::Leaf { key, .. } => {
                                // Get full path of the leaf node
                                child_path.extend(key);
                                &child_path == path
                            }
                            _ => false,
                        })
                        .then_some(nibble);

                    nodes.push(RemovedSparseNode { path: current, node, unset_branch_nibble });

                    current.push_unchecked(nibble);
                }
            }
        }

        Ok(nodes)
    }

    /// Recalculates and updates the RLP hashes of nodes deeper than or equal to the specified
    /// `depth`.
    ///
    /// The root node is considered to be at level 0. This method is useful for optimizing
    /// hash recalculations after localized changes to the trie structure:
    ///
    /// This function identifies all nodes that have changed (based on the prefix set) at the given
    /// depth and recalculates their RLP representation.
    pub fn update_rlp_node_level(&mut self, depth: usize) {
        // Take the current prefix set
        let mut prefix_set = core::mem::take(&mut self.prefix_set).freeze();
        let mut buffers = RlpNodeBuffers::default();

        // Get the nodes that have changed at the given depth.
        let (targets, new_prefix_set) = self.get_changed_nodes_at_depth(&mut prefix_set, depth);
        // Update the prefix set to the prefix set of the nodes that still need to be updated.
        self.prefix_set = new_prefix_set;

        trace!(target: "trie::sparse", ?depth, ?targets, "Updating nodes at depth");

        let mut temp_rlp_buf = core::mem::take(&mut self.rlp_buf);
        for (level, path) in targets {
            buffers.path_stack.push(RlpNodePathStackItem {
                level,
                path,
                is_in_prefix_set: Some(true),
            });
            self.rlp_node(&mut prefix_set, &mut buffers, &mut temp_rlp_buf);
        }
        self.rlp_buf = temp_rlp_buf;
    }

    /// Returns a list of (level, path) tuples identifying the nodes that have changed at the
    /// specified depth, along with a new prefix set for the paths above the provided depth that
    /// remain unchanged.
    ///
    /// Leaf nodes with a depth less than `depth` are returned too.
    ///
    /// This method helps optimize hash recalculations by identifying which specific
    /// nodes need to be updated at each level of the trie.
    ///
    /// # Parameters
    ///
    /// - `prefix_set`: The current prefix set tracking which paths need updates.
    /// - `depth`: The minimum depth (relative to the root) to include nodes in the targets.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - A vector of `(level, Nibbles)` pairs for nodes that require updates at or below the
    ///   specified depth.
    /// - A `PrefixSetMut` containing paths shallower than the specified depth that still need to be
    ///   tracked for future updates.
    fn get_changed_nodes_at_depth(
        &self,
        prefix_set: &mut PrefixSet,
        depth: usize,
    ) -> (Vec<(usize, Nibbles)>, PrefixSetMut) {
        let mut unchanged_prefix_set = PrefixSetMut::default();
        let mut paths = Vec::from([(Nibbles::default(), 0)]);
        let mut targets = Vec::new();

        while let Some((mut path, level)) = paths.pop() {
            match self.nodes.get(&path).unwrap() {
                SparseNode::Empty | SparseNode::Hash(_) => {}
                SparseNode::Leaf { key: _, hash } => {
                    if hash.is_some() && !prefix_set.contains(&path) {
                        continue
                    }

                    targets.push((level, path));
                }
                SparseNode::Extension { key, hash, store_in_db_trie: _ } => {
                    if hash.is_some() && !prefix_set.contains(&path) {
                        continue
                    }

                    if level >= depth {
                        targets.push((level, path));
                    } else {
                        unchanged_prefix_set.insert(path);

                        path.extend(key);
                        paths.push((path, level + 1));
                    }
                }
                SparseNode::Branch { state_mask, hash, store_in_db_trie: _ } => {
                    if hash.is_some() && !prefix_set.contains(&path) {
                        continue
                    }

                    if level >= depth {
                        targets.push((level, path));
                    } else {
                        unchanged_prefix_set.insert(path);

                        for bit in CHILD_INDEX_RANGE.rev() {
                            if state_mask.is_bit_set(bit) {
                                let mut child_path = path;
                                child_path.push_unchecked(bit);
                                paths.push((child_path, level + 1));
                            }
                        }
                    }
                }
            }
        }

        (targets, unchanged_prefix_set)
    }

    /// Look up or calculate the RLP of the node at the root path.
    ///
    /// # Panics
    ///
    /// If the node at provided path does not exist.
    pub fn rlp_node_allocate(&mut self, prefix_set: &mut PrefixSet) -> RlpNode {
        let mut buffers = RlpNodeBuffers::new_with_root_path();
        let mut temp_rlp_buf = core::mem::take(&mut self.rlp_buf);
        let result = self.rlp_node(prefix_set, &mut buffers, &mut temp_rlp_buf);
        self.rlp_buf = temp_rlp_buf;

        result
    }

    /// Looks up or computes the RLP encoding of the node specified by the current
    /// path in the provided buffers.
    ///
    /// The function uses a stack (`RlpNodeBuffers::path_stack`) to track the traversal and
    /// accumulate RLP encodings.
    ///
    /// # Parameters
    ///
    /// - `prefix_set`: The set of trie paths that need their nodes updated.
    /// - `buffers`: The reusable buffers for stack management and temporary RLP values.
    ///
    /// # Panics
    ///
    /// If the node at provided path does not exist.
    pub fn rlp_node(
        &mut self,
        prefix_set: &mut PrefixSet,
        buffers: &mut RlpNodeBuffers,
        rlp_buf: &mut Vec<u8>,
    ) -> RlpNode {
        let _starting_path = buffers.path_stack.last().map(|item| item.path);

        'main: while let Some(RlpNodePathStackItem { level, path, mut is_in_prefix_set }) =
            buffers.path_stack.pop()
        {
            let node = self.nodes.get_mut(&path).unwrap();
            trace!(
                target: "trie::sparse",
                ?_starting_path,
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
                    let mut path = path;
                    path.extend(key);
                    if let Some(hash) = hash.filter(|_| !prefix_set_contains(&path)) {
                        (RlpNode::word_rlp(&hash), SparseNodeType::Leaf)
                    } else {
                        let value = self.values.get(&path).unwrap();
                        rlp_buf.clear();
                        let rlp_node = LeafNodeRef { key, value }.rlp(rlp_buf);
                        *hash = rlp_node.as_hash();
                        (rlp_node, SparseNodeType::Leaf)
                    }
                }
                SparseNode::Extension { key, hash, store_in_db_trie } => {
                    let mut child_path = path;
                    child_path.extend(key);
                    if let Some((hash, store_in_db_trie)) =
                        hash.zip(*store_in_db_trie).filter(|_| !prefix_set_contains(&path))
                    {
                        (
                            RlpNode::word_rlp(&hash),
                            SparseNodeType::Extension { store_in_db_trie: Some(store_in_db_trie) },
                        )
                    } else if buffers.rlp_node_stack.last().is_some_and(|e| e.path == child_path) {
                        let RlpNodeStackItem {
                            path: _,
                            rlp_node: child,
                            node_type: child_node_type,
                        } = buffers.rlp_node_stack.pop().unwrap();
                        rlp_buf.clear();
                        let rlp_node = ExtensionNodeRef::new(key, &child).rlp(rlp_buf);
                        *hash = rlp_node.as_hash();

                        let store_in_db_trie_value = child_node_type.store_in_db_trie();

                        trace!(
                            target: "trie::sparse",
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
                        buffers.path_stack.extend([
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
                        buffers.rlp_node_stack.push(RlpNodeStackItem {
                            path,
                            rlp_node: RlpNode::word_rlp(&hash),
                            node_type: SparseNodeType::Branch {
                                store_in_db_trie: Some(store_in_db_trie),
                            },
                        });
                        continue
                    }
                    let retain_updates = self.updates.is_some() && prefix_set_contains(&path);

                    buffers.branch_child_buf.clear();
                    // Walk children in a reverse order from `f` to `0`, so we pop the `0` first
                    // from the stack and keep walking in the sorted order.
                    for bit in CHILD_INDEX_RANGE.rev() {
                        if state_mask.is_bit_set(bit) {
                            let mut child = path;
                            child.push_unchecked(bit);
                            buffers.branch_child_buf.push(child);
                        }
                    }

                    buffers
                        .branch_value_stack_buf
                        .resize(buffers.branch_child_buf.len(), Default::default());
                    let mut added_children = false;

                    let mut tree_mask = TrieMask::default();
                    let mut hash_mask = TrieMask::default();
                    let mut hashes = Vec::new();
                    for (i, child_path) in buffers.branch_child_buf.iter().enumerate() {
                        if buffers.rlp_node_stack.last().is_some_and(|e| &e.path == child_path) {
                            let RlpNodeStackItem {
                                path: _,
                                rlp_node: child,
                                node_type: child_node_type,
                            } = buffers.rlp_node_stack.pop().unwrap();

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
                            let original_idx = buffers.branch_child_buf.len() - i - 1;
                            buffers.branch_value_stack_buf[original_idx] = child;
                            added_children = true;
                        } else {
                            debug_assert!(!added_children);
                            buffers.path_stack.push(RlpNodePathStackItem {
                                level,
                                path,
                                is_in_prefix_set,
                            });
                            buffers.path_stack.extend(buffers.branch_child_buf.drain(..).map(
                                |path| RlpNodePathStackItem {
                                    level: level + 1,
                                    path,
                                    is_in_prefix_set: None,
                                },
                            ));
                            continue 'main
                        }
                    }

                    trace!(
                        target: "trie::sparse",
                        ?path,
                        ?tree_mask,
                        ?hash_mask,
                        "Branch node masks"
                    );

                    rlp_buf.clear();
                    let branch_node_ref =
                        BranchNodeRef::new(&buffers.branch_value_stack_buf, *state_mask);
                    let rlp_node = branch_node_ref.rlp(rlp_buf);
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
                            updates.updated_nodes.insert(path, branch_node);
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
                            updates.removed_nodes.insert(path);
                        } else if self
                            .branch_node_tree_masks
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
                target: "trie::sparse",
                ?_starting_path,
                ?level,
                ?path,
                ?node,
                ?node_type,
                ?is_in_prefix_set,
                "Added node to rlp node stack"
            );

            buffers.rlp_node_stack.push(RlpNodeStackItem { path, rlp_node, node_type });
        }

        debug_assert_eq!(buffers.rlp_node_stack.len(), 1);
        buffers.rlp_node_stack.pop().unwrap().rlp_node
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
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        store_in_db_trie: Option<bool>,
    },
    /// Sparse branch node with state mask.
    Branch {
        /// The bitmask representing children present in the branch node.
        state_mask: TrieMask,
        /// Pre-computed hash of the sparse node.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
        hash: Option<B256>,
        /// Pre-computed flag indicating whether the trie node should be stored in the database.
        /// Can be reused unless this trie path has been updated.
        ///
        /// If [`None`], then the value is not known and should be calculated from scratch.
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
        Self::Extension { key, hash: None, store_in_db_trie: None }
    }

    /// Create new [`SparseNode::Leaf`] from leaf key and value.
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

    /// Sets the hash of the node for testing purposes.
    ///
    /// For [`SparseNode::Empty`] and [`SparseNode::Hash`] nodes, this method does nothing.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn set_hash(&mut self, new_hash: Option<B256>) {
        match self {
            Self::Empty | Self::Hash(_) => {
                // Cannot set hash for Empty or Hash nodes
            }
            Self::Leaf { hash, .. } | Self::Extension { hash, .. } | Self::Branch { hash, .. } => {
                *hash = new_hash;
            }
        }
    }
}

/// A helper struct used to store information about a node that has been removed
/// during a deletion operation.
#[derive(Debug)]
struct RemovedSparseNode {
    /// The path at which the node was located.
    path: Nibbles,
    /// The removed node
    node: SparseNode,
    /// For branch nodes, an optional nibble that should be unset due to the node being removed.
    ///
    /// During leaf deletion, this identifies the specific branch nibble path that
    /// connects to the leaf being deleted. Then when restructuring the trie after deletion,
    /// this nibble position will be cleared from the branch node's to
    /// indicate that the child no longer exists.
    ///
    /// This is only set for branch nodes that have a direct path to the leaf being deleted.
    unset_branch_nibble: Option<u8>,
}

/// Collection of reusable buffers for [`SerialSparseTrie::rlp_node`] calculations.
///
/// These buffers reduce allocations when computing RLP representations during trie updates.
#[derive(Debug, Default)]
pub struct RlpNodeBuffers {
    /// Stack of RLP node paths
    path_stack: Vec<RlpNodePathStackItem>,
    /// Stack of RLP nodes
    rlp_node_stack: Vec<RlpNodeStackItem>,
    /// Reusable branch child path
    branch_child_buf: SmallVec<[Nibbles; 16]>,
    /// Reusable branch value stack
    branch_value_stack_buf: SmallVec<[RlpNode; 16]>,
}

impl RlpNodeBuffers {
    /// Creates a new instance of buffers with the root path on the stack.
    fn new_with_root_path() -> Self {
        Self {
            path_stack: vec![RlpNodePathStackItem {
                level: 0,
                path: Nibbles::default(),
                is_in_prefix_set: None,
            }],
            rlp_node_stack: Vec::new(),
            branch_child_buf: SmallVec::<[Nibbles; 16]>::new_const(),
            branch_value_stack_buf: SmallVec::<[RlpNode; 16]>::new_const(),
        }
    }
}

/// RLP node path stack item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RlpNodePathStackItem {
    /// Level at which the node is located. Higher numbers correspond to lower levels in the trie.
    pub level: usize,
    /// Path to the node.
    pub path: Nibbles,
    /// Whether the path is in the prefix set. If [`None`], then unknown yet.
    pub is_in_prefix_set: Option<bool>,
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

#[cfg(test)]
mod find_leaf_tests {
    use super::*;
    use crate::blinded::DefaultBlindedProvider;
    use alloy_primitives::map::foldhash::fast::RandomState;
    // Assuming this exists
    use alloy_rlp::Encodable;
    use assert_matches::assert_matches;
    use reth_primitives_traits::Account;
    use reth_trie_common::LeafNode;

    // Helper to create some test values
    fn encode_value(nonce: u64) -> Vec<u8> {
        let account = Account { nonce, ..Default::default() };
        let trie_account = account.into_trie_account(EMPTY_ROOT_HASH);
        let mut buf = Vec::new();
        trie_account.encode(&mut buf);
        buf
    }

    const VALUE_A: fn() -> Vec<u8> = || encode_value(1);
    const VALUE_B: fn() -> Vec<u8> = || encode_value(2);

    #[test]
    fn find_leaf_existing_leaf() {
        // Create a simple trie with one leaf
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let value = b"test_value".to_vec();

        sparse.update_leaf(path, value.clone(), &provider).unwrap();

        // Check that the leaf exists
        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::Exists));

        // Check with expected value matching
        let result = sparse.find_leaf(&path, Some(&value));
        assert_matches!(result, Ok(LeafLookup::Exists));
    }

    #[test]
    fn find_leaf_value_mismatch() {
        // Create a simple trie with one leaf
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        let value = b"test_value".to_vec();
        let wrong_value = b"wrong_value".to_vec();

        sparse.update_leaf(path, value, &provider).unwrap();

        // Check with wrong expected value
        let result = sparse.find_leaf(&path, Some(&wrong_value));
        assert_matches!(
            result,
            Err(LeafLookupError::ValueMismatch { path: p, expected: Some(e), actual: _a }) if p == path && e == wrong_value
        );
    }

    #[test]
    fn find_leaf_not_found_empty_trie() {
        // Empty trie
        let sparse = SerialSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);

        // Leaf should not exist
        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_empty_trie() {
        let sparse = SerialSparseTrie::default();
        let path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_exists_no_value_check() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);
        sparse.update_leaf(path, VALUE_A(), &provider).unwrap();

        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::Exists));
    }

    #[test]
    fn find_leaf_exists_with_value_check_ok() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);
        let value = VALUE_A();
        sparse.update_leaf(path, value.clone(), &provider).unwrap();

        let result = sparse.find_leaf(&path, Some(&value));
        assert_matches!(result, Ok(LeafLookup::Exists));
    }

    #[test]
    fn find_leaf_exclusion_branch_divergence() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let path1 = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]); // Creates branch at 0x12
        let path2 = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x5, 0x6]); // Belongs to same branch
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x7, 0x8]); // Diverges at nibble 7

        sparse.update_leaf(path1, VALUE_A(), &provider).unwrap();
        sparse.update_leaf(path2, VALUE_B(), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_exclusion_extension_divergence() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        // This will create an extension node at root with key 0x12
        let path1 = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4, 0x5, 0x6]);
        // This path diverges from the extension key
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x7, 0x8]);

        sparse.update_leaf(path1, VALUE_A(), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_exclusion_leaf_divergence() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let existing_leaf_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4, 0x5, 0x6]);

        sparse.update_leaf(existing_leaf_path, VALUE_A(), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_exclusion_path_ends_at_branch() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let path1 = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]); // Creates branch at 0x12
        let path2 = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x5, 0x6]);
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2]); // Path of the branch itself

        sparse.update_leaf(path1, VALUE_A(), &provider).unwrap();
        sparse.update_leaf(path2, VALUE_B(), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_error_blinded_node_at_leaf_path() {
        // Scenario: The node *at* the leaf path is blinded.
        let blinded_hash = B256::repeat_byte(0xBB);
        let leaf_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

        let mut nodes = alloy_primitives::map::HashMap::with_hasher(RandomState::default());
        // Create path to the blinded node
        nodes.insert(
            Nibbles::default(),
            SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x1, 0x2])),
        ); // Ext 0x12
        nodes.insert(
            Nibbles::from_nibbles_unchecked([0x1, 0x2]),
            SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x3])),
        ); // Ext 0x123
        nodes.insert(
            Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3]),
            SparseNode::new_branch(TrieMask::new(0b10000)),
        ); // Branch at 0x123, child 4
        nodes.insert(leaf_path, SparseNode::Hash(blinded_hash)); // Blinded node at 0x1234

        let sparse = SerialSparseTrie {
            nodes,
            branch_node_tree_masks: Default::default(),
            branch_node_hash_masks: Default::default(),
            /* The value is not in the values map, or else it would early return */
            values: Default::default(),
            prefix_set: Default::default(),
            updates: None,
            rlp_buf: Vec::new(),
        };

        let result = sparse.find_leaf(&leaf_path, None);

        // Should error because it hit the blinded node exactly at the leaf path
        assert_matches!(result, Err(LeafLookupError::BlindedNode { path, hash })
            if path == leaf_path && hash == blinded_hash
        );
    }

    #[test]
    fn find_leaf_error_blinded_node() {
        let blinded_hash = B256::repeat_byte(0xAA);
        let path_to_blind = Nibbles::from_nibbles_unchecked([0x1]);
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

        let mut nodes = HashMap::with_hasher(RandomState::default());

        // Root is a branch with child 0x1 (blinded) and 0x5 (revealed leaf)
        // So we set Bit 1 and Bit 5 in the state_mask
        let state_mask = TrieMask::new(0b100010);
        nodes.insert(Nibbles::default(), SparseNode::new_branch(state_mask));

        nodes.insert(path_to_blind, SparseNode::Hash(blinded_hash));
        let path_revealed = Nibbles::from_nibbles_unchecked([0x5]);
        let path_revealed_leaf = Nibbles::from_nibbles_unchecked([0x5, 0x6, 0x7, 0x8]);
        nodes.insert(
            path_revealed,
            SparseNode::new_leaf(Nibbles::from_nibbles_unchecked([0x6, 0x7, 0x8])),
        );

        let mut values = HashMap::with_hasher(RandomState::default());
        values.insert(path_revealed_leaf, VALUE_A());

        let sparse = SerialSparseTrie {
            nodes,
            branch_node_tree_masks: Default::default(),
            branch_node_hash_masks: Default::default(),
            values,
            prefix_set: Default::default(),
            updates: None,
            rlp_buf: Vec::new(),
        };

        let result = sparse.find_leaf(&search_path, None);

        // Should error because it hit the blinded node at path 0x1
        assert_matches!(result, Err(LeafLookupError::BlindedNode { path, hash })
            if path == path_to_blind && hash == blinded_hash
        );
    }

    #[test]
    fn find_leaf_error_blinded_node_via_reveal() {
        let blinded_hash = B256::repeat_byte(0xAA);
        let path_to_blind = Nibbles::from_nibbles_unchecked([0x1]); // Path of the blinded node itself
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]); // Path we will search for

        let revealed_leaf_prefix = Nibbles::from_nibbles_unchecked([0x5]);
        let revealed_leaf_suffix = Nibbles::from_nibbles_unchecked([0x6, 0x7, 0x8]);
        let revealed_leaf_full_path = Nibbles::from_nibbles_unchecked([0x5, 0x6, 0x7, 0x8]);
        let revealed_value = VALUE_A();

        // 1. Construct the RLP representation of the children for the root branch
        let rlp_node_child1 = RlpNode::word_rlp(&blinded_hash); // Blinded node

        let leaf_node_child5 = LeafNode::new(revealed_leaf_suffix, revealed_value.clone());
        let leaf_node_child5_rlp_buf = alloy_rlp::encode(&leaf_node_child5);
        let hash_of_child5 = keccak256(&leaf_node_child5_rlp_buf);
        let rlp_node_child5 = RlpNode::word_rlp(&hash_of_child5);

        // 2. Construct the root BranchNode using the RLP of its children
        // The stack order depends on the bit indices (1 and 5)
        let root_branch_node = reth_trie_common::BranchNode::new(
            vec![rlp_node_child1, rlp_node_child5], // Child 1 first, then Child 5
            TrieMask::new(0b100010),                // Mask with bits 1 and 5 set
        );
        let root_trie_node = TrieNode::Branch(root_branch_node);

        // 3. Initialize the sparse trie using from_root
        // This will internally create Hash nodes for paths "1" and "5" initially.
        let mut sparse = SerialSparseTrie::from_root(root_trie_node, TrieMasks::none(), false)
            .expect("Failed to create trie from root");

        // Assertions before we reveal child5
        assert_matches!(sparse.nodes.get(&Nibbles::default()), Some(SparseNode::Branch { state_mask, .. }) if *state_mask == TrieMask::new(0b100010)); // Here we check that 1 and 5 are set in the state_mask
        assert_matches!(sparse.nodes.get(&path_to_blind), Some(SparseNode::Hash(h)) if *h == blinded_hash );
        assert!(sparse.nodes.get(&revealed_leaf_prefix).unwrap().is_hash()); // Child 5 is initially a hash of its RLP
        assert!(sparse.values.is_empty());

        // 4. Explicitly reveal the leaf node for child 5
        sparse
            .reveal_node(revealed_leaf_prefix, TrieNode::Leaf(leaf_node_child5), TrieMasks::none())
            .expect("Failed to reveal leaf node");

        // Assertions after we reveal child 5
        assert_matches!(sparse.nodes.get(&Nibbles::default()), Some(SparseNode::Branch { state_mask, .. }) if *state_mask == TrieMask::new(0b100010));
        assert_matches!(sparse.nodes.get(&path_to_blind), Some(SparseNode::Hash(h)) if *h == blinded_hash );
        assert_matches!(sparse.nodes.get(&revealed_leaf_prefix), Some(SparseNode::Leaf { key, .. }) if *key == revealed_leaf_suffix);
        assert_eq!(sparse.values.get(&revealed_leaf_full_path), Some(&revealed_value));

        let result = sparse.find_leaf(&search_path, None);

        // 5. Assert the expected error
        // Should error because it hit the blinded node at path "1" only node at "5" was revealed
        assert_matches!(result, Err(LeafLookupError::BlindedNode { path, hash })
            if path == path_to_blind && hash == blinded_hash
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blinded::DefaultBlindedProvider;
    use alloy_primitives::{map::B256Set, U256};
    use alloy_rlp::Encodable;
    use assert_matches::assert_matches;
    use itertools::Itertools;
    use prop::sample::SizeRange;
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb;
    use reth_primitives_traits::Account;
    use reth_provider::{test_utils::create_test_provider_factory, TrieWriter};
    use reth_trie::{
        hashed_cursor::{noop::NoopHashedAccountCursor, HashedPostStateAccountCursor},
        node_iter::{TrieElement, TrieNodeIter},
        trie_cursor::{noop::NoopAccountTrieCursor, TrieCursor, TrieCursorFactory},
        walker::TrieWalker,
        BranchNode, ExtensionNode, HashedPostState, LeafNode,
    };
    use reth_trie_common::{
        proof::{ProofNodes, ProofRetainer},
        updates::TrieUpdates,
        HashBuilder,
    };
    use reth_trie_db::DatabaseTrieCursorFactory;
    use std::collections::{BTreeMap, BTreeSet};

    /// Pad nibbles to the length of a B256 hash with zeros on the left.
    fn pad_nibbles_left(nibbles: Nibbles) -> Nibbles {
        let mut base =
            Nibbles::from_nibbles_unchecked(vec![0; B256::len_bytes() * 2 - nibbles.len()]);
        base.extend(&nibbles);
        base
    }

    /// Pad nibbles to the length of a B256 hash with zeros on the right.
    fn pad_nibbles_right(mut nibbles: Nibbles) -> Nibbles {
        nibbles.extend(&Nibbles::from_nibbles_unchecked(vec![
            0;
            B256::len_bytes() * 2 - nibbles.len()
        ]));
        nibbles
    }

    /// Calculate the state root by feeding the provided state to the hash builder and retaining the
    /// proofs for the provided targets.
    ///
    /// Returns the state root and the retained proof nodes.
    fn run_hash_builder(
        state: impl IntoIterator<Item = (Nibbles, Account)> + Clone,
        trie_cursor: impl TrieCursor,
        destroyed_accounts: B256Set,
        proof_targets: impl IntoIterator<Item = Nibbles>,
    ) -> (B256, TrieUpdates, ProofNodes, HashMap<Nibbles, TrieMask>, HashMap<Nibbles, TrieMask>)
    {
        let mut account_rlp = Vec::new();

        let mut hash_builder = HashBuilder::default()
            .with_updates(true)
            .with_proof_retainer(ProofRetainer::from_iter(proof_targets));

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.extend_keys(state.clone().into_iter().map(|(nibbles, _)| nibbles));
        prefix_set.extend_keys(destroyed_accounts.iter().map(Nibbles::unpack));
        let walker =
            TrieWalker::state_trie(trie_cursor, prefix_set.freeze()).with_deletions_retained(true);
        let hashed_post_state = HashedPostState::default()
            .with_accounts(state.into_iter().map(|(nibbles, account)| {
                (nibbles.pack().into_inner().unwrap().into(), Some(account))
            }))
            .into_sorted();
        let mut node_iter = TrieNodeIter::state_trie(
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
                    let account = account.into_trie_account(EMPTY_ROOT_HASH);
                    account.encode(&mut account_rlp);

                    hash_builder.add_leaf(Nibbles::unpack(key), &account_rlp);
                    account_rlp.clear();
                }
            }
        }
        let root = hash_builder.root();
        let proof_nodes = hash_builder.take_proof_nodes();
        let branch_node_hash_masks = hash_builder
            .updated_branch_nodes
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|(path, node)| (*path, node.hash_mask))
            .collect();
        let branch_node_tree_masks = hash_builder
            .updated_branch_nodes
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|(path, node)| (*path, node.tree_mask))
            .collect();

        let mut trie_updates = TrieUpdates::default();
        let removed_keys = node_iter.walker.take_removed_keys();
        trie_updates.finalize(hash_builder, removed_keys, destroyed_accounts);

        (root, trie_updates, proof_nodes, branch_node_hash_masks, branch_node_tree_masks)
    }

    /// Assert that the sparse trie nodes and the proof nodes from the hash builder are equal.
    fn assert_eq_sparse_trie_proof_nodes(sparse_trie: &SerialSparseTrie, proof_nodes: ProofNodes) {
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
            assert!(
                equals,
                "path: {proof_node_path:?}\nproof node: {proof_node:?}\nsparse node: {sparse_node:?}"
            );
        }
    }

    #[test]
    fn sparse_trie_is_blind() {
        assert!(SparseTrie::<SerialSparseTrie>::blind().is_blind());
        assert!(!SparseTrie::<SerialSparseTrie>::revealed_empty().is_blind());
    }

    #[test]
    fn sparse_trie_empty_update_one() {
        let key = Nibbles::unpack(B256::with_last_byte(42));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                [(key, value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key],
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default().with_updates(true);
        sparse.update_leaf(key, value_encoded(), &provider).unwrap();
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
    }

    #[test]
    fn sparse_trie_empty_update_multiple_lower_nibbles() {
        reth_tracing::init_test_tracing();

        let paths = (0..=16).map(|b| Nibbles::unpack(B256::with_last_byte(b))).collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(std::iter::repeat_with(value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(*path, value_encoded(), &provider).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
    }

    #[test]
    fn sparse_trie_empty_update_multiple_upper_nibbles() {
        let paths = (239..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(std::iter::repeat_with(value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(*path, value_encoded(), &provider).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
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
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().sorted_unstable().copied().zip(std::iter::repeat_with(value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(*path, value_encoded(), &provider).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        pretty_assertions::assert_eq!(
            BTreeMap::from_iter(sparse_updates.updated_nodes),
            BTreeMap::from_iter(hash_builder_updates.account_nodes)
        );
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
    }

    #[test]
    fn sparse_trie_empty_update_repeated() {
        let paths = (0..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let old_value = Account { nonce: 1, ..Default::default() };
        let old_value_encoded = {
            let mut account_rlp = Vec::new();
            old_value.into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };
        let new_value = Account { nonce: 2, ..Default::default() };
        let new_value_encoded = {
            let mut account_rlp = Vec::new();
            new_value.into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(std::iter::repeat_with(|| old_value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(*path, old_value_encoded.clone(), &provider).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.updates_ref();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(std::iter::repeat_with(|| new_value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        for path in &paths {
            sparse.update_leaf(*path, new_value_encoded.clone(), &provider).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
    }

    #[test]
    fn sparse_trie_remove_leaf() {
        reth_tracing::init_test_tracing();

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();

        let value = alloy_rlp::encode_fixed_size(&U256::ZERO).to_vec();

        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value, &provider)
            .unwrap();

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

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), &provider).unwrap();

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

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), &provider).unwrap();

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

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), &provider).unwrap();

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

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), &provider).unwrap();

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

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), &provider).unwrap();

        // Leaf (Key = 53302)
        pretty_assertions::assert_eq!(
            sparse.nodes.clone().into_iter().collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([(
                Nibbles::default(),
                SparseNode::new_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]))
            ),])
        );

        sparse.remove_leaf(&Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), &provider).unwrap();

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

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::from_root(
            branch.clone(),
            TrieMasks { hash_mask: Some(TrieMask::new(0b01)), tree_mask: None },
            false,
        )
        .unwrap();

        // Reveal a branch node and one of its children
        //
        // Branch (Mask = 11)
        // ├── 0 -> Hash (Path = 0)
        // └── 1 -> Leaf (Path = 1)
        sparse
            .reveal_node(
                Nibbles::default(),
                branch,
                TrieMasks { hash_mask: None, tree_mask: Some(TrieMask::new(0b01)) },
            )
            .unwrap();
        sparse
            .reveal_node(Nibbles::from_nibbles([0x1]), TrieNode::Leaf(leaf), TrieMasks::none())
            .unwrap();

        // Removing a blinded leaf should result in an error
        assert_matches!(
            sparse.remove_leaf(&Nibbles::from_nibbles([0x0]), &provider).map_err(|e| e.into_kind()),
            Err(SparseTrieErrorKind::BlindedNode { path, hash }) if path == Nibbles::from_nibbles([0x0]) && hash == B256::repeat_byte(1)
        );
    }

    #[test]
    fn sparse_trie_remove_leaf_non_existent() {
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

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::from_root(
            branch.clone(),
            TrieMasks { hash_mask: Some(TrieMask::new(0b01)), tree_mask: None },
            false,
        )
        .unwrap();

        // Reveal a branch node and one of its children
        //
        // Branch (Mask = 11)
        // ├── 0 -> Hash (Path = 0)
        // └── 1 -> Leaf (Path = 1)
        sparse
            .reveal_node(
                Nibbles::default(),
                branch,
                TrieMasks { hash_mask: None, tree_mask: Some(TrieMask::new(0b01)) },
            )
            .unwrap();
        sparse
            .reveal_node(Nibbles::from_nibbles([0x1]), TrieNode::Leaf(leaf), TrieMasks::none())
            .unwrap();

        // Removing a non-existent leaf should be a noop
        let sparse_old = sparse.clone();
        assert_matches!(sparse.remove_leaf(&Nibbles::from_nibbles([0x2]), &provider), Ok(()));
        assert_eq!(sparse, sparse_old);
    }

    #[test]
    fn sparse_trie_fuzz() {
        // Having only the first 3 nibbles set, we narrow down the range of keys
        // to 4096 different hashes. It allows us to generate collisions more likely
        // to test the sparse trie updates.
        const KEY_NIBBLES_LEN: usize = 3;

        fn test(updates: Vec<(BTreeMap<Nibbles, Account>, BTreeSet<Nibbles>)>) {
            {
                let mut state = BTreeMap::default();
                let default_provider = DefaultBlindedProvider;
                let provider_factory = create_test_provider_factory();
                let mut sparse = SerialSparseTrie::default().with_updates(true);

                for (update, keys_to_delete) in updates {
                    // Insert state updates into the sparse trie and calculate the root
                    for (key, account) in update.clone() {
                        let account = account.into_trie_account(EMPTY_ROOT_HASH);
                        let mut account_rlp = Vec::new();
                        account.encode(&mut account_rlp);
                        sparse.update_leaf(key, account_rlp, &default_provider).unwrap();
                    }
                    // We need to clone the sparse trie, so that all updated branch nodes are
                    // preserved, and not only those that were changed after the last call to
                    // `root()`.
                    let mut updated_sparse = sparse.clone();
                    let sparse_root = updated_sparse.root();
                    let sparse_updates = updated_sparse.take_updates();

                    // Insert state updates into the hash builder and calculate the root
                    state.extend(update);
                    let provider = provider_factory.provider().unwrap();
                    let trie_cursor = DatabaseTrieCursorFactory::new(provider.tx_ref());
                    let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
                        run_hash_builder(
                            state.clone(),
                            trie_cursor.account_trie_cursor().unwrap(),
                            Default::default(),
                            state.keys().copied().collect::<Vec<_>>(),
                        );

                    // Write trie updates to the database
                    let provider_rw = provider_factory.provider_rw().unwrap();
                    provider_rw.write_trie_updates(&hash_builder_updates).unwrap();
                    provider_rw.commit().unwrap();

                    // Assert that the sparse trie root matches the hash builder root
                    assert_eq!(sparse_root, hash_builder_root);
                    // Assert that the sparse trie updates match the hash builder updates
                    pretty_assertions::assert_eq!(
                        BTreeMap::from_iter(sparse_updates.updated_nodes),
                        BTreeMap::from_iter(hash_builder_updates.account_nodes)
                    );
                    // Assert that the sparse trie nodes match the hash builder proof nodes
                    assert_eq_sparse_trie_proof_nodes(&updated_sparse, hash_builder_proof_nodes);

                    // Delete some keys from both the hash builder and the sparse trie and check
                    // that the sparse trie root still matches the hash builder root
                    for key in &keys_to_delete {
                        state.remove(key).unwrap();
                        sparse.remove_leaf(key, &default_provider).unwrap();
                    }

                    // We need to clone the sparse trie, so that all updated branch nodes are
                    // preserved, and not only those that were changed after the last call to
                    // `root()`.
                    let mut updated_sparse = sparse.clone();
                    let sparse_root = updated_sparse.root();
                    let sparse_updates = updated_sparse.take_updates();

                    let provider = provider_factory.provider().unwrap();
                    let trie_cursor = DatabaseTrieCursorFactory::new(provider.tx_ref());
                    let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
                        run_hash_builder(
                            state.clone(),
                            trie_cursor.account_trie_cursor().unwrap(),
                            keys_to_delete
                                .iter()
                                .map(|nibbles| B256::from_slice(&nibbles.pack()))
                                .collect(),
                            state.keys().copied().collect::<Vec<_>>(),
                        );

                    // Write trie updates to the database
                    let provider_rw = provider_factory.provider_rw().unwrap();
                    provider_rw.write_trie_updates(&hash_builder_updates).unwrap();
                    provider_rw.commit().unwrap();

                    // Assert that the sparse trie root matches the hash builder root
                    assert_eq!(sparse_root, hash_builder_root);
                    // Assert that the sparse trie updates match the hash builder updates
                    pretty_assertions::assert_eq!(
                        BTreeMap::from_iter(sparse_updates.updated_nodes),
                        BTreeMap::from_iter(hash_builder_updates.account_nodes)
                    );
                    // Assert that the sparse trie nodes match the hash builder proof nodes
                    assert_eq_sparse_trie_proof_nodes(&updated_sparse, hash_builder_proof_nodes);
                }
            }
        }

        fn transform_updates(
            updates: Vec<BTreeMap<Nibbles, Account>>,
            mut rng: impl rand::Rng,
        ) -> Vec<(BTreeMap<Nibbles, Account>, BTreeSet<Nibbles>)> {
            let mut keys = BTreeSet::new();
            updates
                .into_iter()
                .map(|update| {
                    keys.extend(update.keys().copied());

                    let keys_to_delete_len = update.len() / 2;
                    let keys_to_delete = (0..keys_to_delete_len)
                        .map(|_| {
                            let key =
                                *rand::seq::IteratorRandom::choose(keys.iter(), &mut rng).unwrap();
                            keys.take(&key).unwrap()
                        })
                        .collect();

                    (update, keys_to_delete)
                })
                .collect::<Vec<_>>()
        }

        proptest!(ProptestConfig::with_cases(10), |(
            updates in proptest::collection::vec(
                proptest::collection::btree_map(
                    any_with::<Nibbles>(SizeRange::new(KEY_NIBBLES_LEN..=KEY_NIBBLES_LEN)).prop_map(pad_nibbles_right),
                    arb::<Account>(),
                    1..50,
                ),
                1..50,
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
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        // Generate the proof for the root node and initialize the sparse trie with it
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [Nibbles::default()],
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::from_root(
            TrieNode::decode(&mut &hash_builder_proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            TrieMasks {
                hash_mask: branch_node_hash_masks.get(&Nibbles::default()).copied(),
                tree_mask: branch_node_tree_masks.get(&Nibbles::default()).copied(),
            },
            false,
        )
        .unwrap();

        // Generate the proof for the first key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key1()],
            );
        for (path, node) in hash_builder_proof_nodes.nodes_sorted() {
            let hash_mask = branch_node_hash_masks.get(&path).copied();
            let tree_mask = branch_node_tree_masks.get(&path).copied();
            sparse
                .reveal_node(
                    path,
                    TrieNode::decode(&mut &node[..]).unwrap(),
                    TrieMasks { hash_mask, tree_mask },
                )
                .unwrap();
        }

        // Check that the branch node exists with only two nibbles set
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b101.into()))
        );

        // Insert the leaf for the second key
        sparse.update_leaf(key2(), value_encoded(), &provider).unwrap();

        // Check that the branch node was updated and another nibble was set
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b111.into()))
        );

        // Generate the proof for the third key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key3()],
            );
        for (path, node) in hash_builder_proof_nodes.nodes_sorted() {
            let hash_mask = branch_node_hash_masks.get(&path).copied();
            let tree_mask = branch_node_tree_masks.get(&path).copied();
            sparse
                .reveal_node(
                    path,
                    TrieNode::decode(&mut &node[..]).unwrap(),
                    TrieMasks { hash_mask, tree_mask },
                )
                .unwrap();
        }

        // Check that nothing changed in the branch node
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b111.into()))
        );

        // Generate the nodes for the full trie with all three key using the hash builder, and
        // compare them to the sparse trie
        let (_, _, hash_builder_proof_nodes, _, _) = run_hash_builder(
            [(key1(), value()), (key2(), value()), (key3(), value())],
            NoopAccountTrieCursor::default(),
            Default::default(),
            [key1(), key2(), key3()],
        );

        assert_eq_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
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
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [Nibbles::default()],
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::from_root(
            TrieNode::decode(&mut &hash_builder_proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            TrieMasks {
                hash_mask: branch_node_hash_masks.get(&Nibbles::default()).copied(),
                tree_mask: branch_node_tree_masks.get(&Nibbles::default()).copied(),
            },
            false,
        )
        .unwrap();

        // Generate the proof for the children of the root branch node and reveal it in the sparse
        // trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key1(), Nibbles::from_nibbles_unchecked([0x01])],
            );
        for (path, node) in hash_builder_proof_nodes.nodes_sorted() {
            let hash_mask = branch_node_hash_masks.get(&path).copied();
            let tree_mask = branch_node_tree_masks.get(&path).copied();
            sparse
                .reveal_node(
                    path,
                    TrieNode::decode(&mut &node[..]).unwrap(),
                    TrieMasks { hash_mask, tree_mask },
                )
                .unwrap();
        }

        // Check that the branch node exists
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(0b11.into()))
        );

        // Remove the leaf for the first key
        sparse.remove_leaf(&key1(), &provider).unwrap();

        // Check that the branch node was turned into an extension node
        assert_eq!(
            sparse.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x01])))
        );

        // Generate the proof for the third key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key2()],
            );
        for (path, node) in hash_builder_proof_nodes.nodes_sorted() {
            let hash_mask = branch_node_hash_masks.get(&path).copied();
            let tree_mask = branch_node_tree_masks.get(&path).copied();
            sparse
                .reveal_node(
                    path,
                    TrieNode::decode(&mut &node[..]).unwrap(),
                    TrieMasks { hash_mask, tree_mask },
                )
                .unwrap();
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
    /// 1. Insert the leaf 0x0100 into the sparse trie, and check that the root extension node was
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
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        // Generate the proof for the root node and initialize the sparse trie with it
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [Nibbles::default()],
            );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::from_root(
            TrieNode::decode(&mut &hash_builder_proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            TrieMasks {
                hash_mask: branch_node_hash_masks.get(&Nibbles::default()).copied(),
                tree_mask: branch_node_tree_masks.get(&Nibbles::default()).copied(),
            },
            false,
        )
        .unwrap();

        // Check that the root extension node exists
        assert_matches!(
            sparse.nodes.get(&Nibbles::default()),
            Some(SparseNode::Extension { key, hash: None, store_in_db_trie: None }) if *key == Nibbles::from_nibbles([0x00])
        );

        // Insert the leaf with a different prefix
        sparse.update_leaf(key3(), value_encoded(), &provider).unwrap();

        // Check that the extension node was turned into a branch node
        assert_matches!(
            sparse.nodes.get(&Nibbles::default()),
            Some(SparseNode::Branch { state_mask, hash: None, store_in_db_trie: None }) if *state_mask == TrieMask::new(0b11)
        );

        // Generate the proof for the first key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key1()],
            );
        for (path, node) in hash_builder_proof_nodes.nodes_sorted() {
            let hash_mask = branch_node_hash_masks.get(&path).copied();
            let tree_mask = branch_node_tree_masks.get(&path).copied();
            sparse
                .reveal_node(
                    path,
                    TrieNode::decode(&mut &node[..]).unwrap(),
                    TrieMasks { hash_mask, tree_mask },
                )
                .unwrap();
        }

        // Check that the branch node wasn't overwritten by the extension node in the proof
        assert_matches!(
            sparse.nodes.get(&Nibbles::default()),
            Some(SparseNode::Branch { state_mask, hash: None, store_in_db_trie: None }) if *state_mask == TrieMask::new(0b11)
        );
    }

    #[test]
    fn sparse_trie_get_changed_nodes_at_depth() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();

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
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value, &provider)
            .unwrap();

        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 0),
            (vec![(0, Nibbles::default())], PrefixSetMut::default())
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 1),
            (vec![(1, Nibbles::from_nibbles_unchecked([0x5]))], [Nibbles::default()].into())
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 2),
            (
                vec![
                    (2, Nibbles::from_nibbles_unchecked([0x5, 0x0])),
                    (2, Nibbles::from_nibbles_unchecked([0x5, 0x2])),
                    (2, Nibbles::from_nibbles_unchecked([0x5, 0x3]))
                ],
                [Nibbles::default(), Nibbles::from_nibbles_unchecked([0x5])].into()
            )
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 3),
            (
                vec![
                    (3, Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3])),
                    (2, Nibbles::from_nibbles_unchecked([0x5, 0x2])),
                    (3, Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x1])),
                    (3, Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3]))
                ],
                [
                    Nibbles::default(),
                    Nibbles::from_nibbles_unchecked([0x5]),
                    Nibbles::from_nibbles_unchecked([0x5, 0x0]),
                    Nibbles::from_nibbles_unchecked([0x5, 0x3])
                ]
                .into()
            )
        );
        assert_eq!(
            sparse.get_changed_nodes_at_depth(&mut PrefixSet::default(), 4),
            (
                vec![
                    (4, Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3, 0x1])),
                    (4, Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3, 0x3])),
                    (2, Nibbles::from_nibbles_unchecked([0x5, 0x2])),
                    (3, Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x1])),
                    (4, Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3, 0x0])),
                    (4, Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3, 0x2]))
                ],
                [
                    Nibbles::default(),
                    Nibbles::from_nibbles_unchecked([0x5]),
                    Nibbles::from_nibbles_unchecked([0x5, 0x0]),
                    Nibbles::from_nibbles_unchecked([0x5, 0x0, 0x2, 0x3]),
                    Nibbles::from_nibbles_unchecked([0x5, 0x3]),
                    Nibbles::from_nibbles_unchecked([0x5, 0x3, 0x3])
                ]
                .into()
            )
        );
    }

    #[test]
    fn hash_builder_branch_hash_mask() {
        let key1 = || pad_nibbles_left(Nibbles::from_nibbles_unchecked([0x00]));
        let key2 = || pad_nibbles_left(Nibbles::from_nibbles_unchecked([0x01]));
        let value = || Account { bytecode_hash: Some(B256::repeat_byte(1)), ..Default::default() };
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, _, _, _) = run_hash_builder(
            [(key1(), value()), (key2(), value())],
            NoopAccountTrieCursor::default(),
            Default::default(),
            [Nibbles::default()],
        );

        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        sparse.update_leaf(key1(), value_encoded(), &provider).unwrap();
        sparse.update_leaf(key2(), value_encoded(), &provider).unwrap();
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
    }

    #[test]
    fn sparse_trie_wipe() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default().with_updates(true);

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
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value, &provider)
            .unwrap();

        sparse.wipe();

        assert_matches!(
            &sparse.updates,
            Some(SparseTrieUpdates{ updated_nodes, removed_nodes, wiped })
            if updated_nodes.is_empty() && removed_nodes.is_empty() && *wiped
        );
        assert_eq!(sparse.root(), EMPTY_ROOT_HASH);
    }

    #[test]
    fn sparse_trie_clear() {
        // tests that if we fill a sparse trie with some nodes and then clear it, it has the same
        // contents as an empty sparse trie
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();
        let value = alloy_rlp::encode_fixed_size(&U256::ZERO).to_vec();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value, &provider)
            .unwrap();

        sparse.clear();

        let empty_trie = SerialSparseTrie::default();
        assert_eq!(empty_trie, sparse);
    }

    #[test]
    fn sparse_trie_display() {
        let provider = DefaultBlindedProvider;
        let mut sparse = SerialSparseTrie::default();

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
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2]), value.clone(), &provider)
            .unwrap();
        sparse
            .update_leaf(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0]), value, &provider)
            .unwrap();

        let normal_printed = format!("{sparse}");
        let expected = "\
Root -> Extension { key: Nibbles(0x5), hash: None, store_in_db_trie: None }
5 -> Branch { state_mask: TrieMask(0000000000001101), hash: None, store_in_db_trie: None }
50 -> Extension { key: Nibbles(0x23), hash: None, store_in_db_trie: None }
5023 -> Branch { state_mask: TrieMask(0000000000001010), hash: None, store_in_db_trie: None }
50231 -> Leaf { key: Nibbles(0x), hash: None }
50233 -> Leaf { key: Nibbles(0x), hash: None }
52013 -> Leaf { key: Nibbles(0x013), hash: None }
53 -> Branch { state_mask: TrieMask(0000000000001010), hash: None, store_in_db_trie: None }
53102 -> Leaf { key: Nibbles(0x02), hash: None }
533 -> Branch { state_mask: TrieMask(0000000000000101), hash: None, store_in_db_trie: None }
53302 -> Leaf { key: Nibbles(0x2), hash: None }
53320 -> Leaf { key: Nibbles(0x0), hash: None }
";
        assert_eq!(normal_printed, expected);

        let alternate_printed = format!("{sparse:#}");
        let expected = "\
Root -> Extension { key: Nibbles(0x5), hash: None, store_in_db_trie: None }
    5 -> Branch { state_mask: TrieMask(0000000000001101), hash: None, store_in_db_trie: None }
        50 -> Extension { key: Nibbles(0x23), hash: None, store_in_db_trie: None }
            5023 -> Branch { state_mask: TrieMask(0000000000001010), hash: None, store_in_db_trie: None }
                50231 -> Leaf { key: Nibbles(0x), hash: None }
                50233 -> Leaf { key: Nibbles(0x), hash: None }
        52013 -> Leaf { key: Nibbles(0x013), hash: None }
        53 -> Branch { state_mask: TrieMask(0000000000001010), hash: None, store_in_db_trie: None }
            53102 -> Leaf { key: Nibbles(0x02), hash: None }
            533 -> Branch { state_mask: TrieMask(0000000000000101), hash: None, store_in_db_trie: None }
                53302 -> Leaf { key: Nibbles(0x2), hash: None }
                53320 -> Leaf { key: Nibbles(0x0), hash: None }
";

        assert_eq!(alternate_printed, expected);
    }
}
