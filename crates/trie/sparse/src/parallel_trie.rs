use crate::{SparseNode, SparseTrieUpdates, TrieMasks};
use alloc::vec::Vec;
use alloy_primitives::{
    map::{Entry, HashMap},
    B256,
};
use alloy_rlp::Decodable;
use reth_execution_errors::{SparseTrieErrorKind, SparseTrieResult};
use reth_trie_common::{Nibbles, TrieNode, CHILD_INDEX_RANGE};

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
    lower_subtries: [Option<SparseSubtrie>; 256],
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            upper_subtrie: SparseSubtrie::default(),
            lower_subtries: [const { None }; 256],
            updates: None,
        }
    }
}

impl ParallelSparseTrie {
    /// Returns mutable ref to the lower `SparseSubtrie` for the given path, or None if the path
    /// belongs to the upper trie.
    fn lower_subtrie_for_path(&mut self, path: &Nibbles) -> Option<&mut SparseSubtrie> {
        match SparseSubtrieType::from_path(path) {
            SparseSubtrieType::Upper => None,
            SparseSubtrieType::Lower(idx) => {
                if self.lower_subtries[idx].is_none() {
                    let upper_path = path.slice(..2);
                    self.lower_subtries[idx] = Some(SparseSubtrie::new(upper_path));
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
}

/// This is a subtrie of the `ParallelSparseTrie` that contains a map from path to sparse trie
/// nodes.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SparseSubtrie {
    /// The root path of this subtrie.
    path: Nibbles,
    /// The map from paths to sparse trie nodes within this subtrie.
    nodes: HashMap<Nibbles, SparseNode>,
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
}

impl SparseSubtrie {
    fn new(path: Nibbles) -> Self {
        Self { path, ..Default::default() }
    }

    /// Returns true if the current path and its child are both found in the same level. This
    /// function assumes that if current_path is in a lower level that the child_path is too.
    fn is_child_same_level(current_path: &Nibbles, child_path: &Nibbles) -> bool {
        !matches!(SparseSubtrieType::from_path(current_path), SparseSubtrieType::Upper) ||
            !matches!(SparseSubtrieType::from_path(child_path), SparseSubtrieType::Lower(_))
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
            // Convert first two nibbles of the path into a number.
            let index = (path[0] << 4 | path[1]) as usize;
            Self::Lower(index)
        }
    }
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
    use crate::{
        parallel_trie::{path_subtrie_index_unchecked, ParallelSparseTrie, SparseSubtrieType},
        SparseNode, TrieMasks,
    };
    use alloy_primitives::B256;
    use alloy_rlp::Encodable;
    use alloy_trie::Nibbles;
    use assert_matches::assert_matches;
    use reth_primitives_traits::Account;
    use reth_trie_common::{
        BranchNode, ExtensionNode, LeafNode, RlpNode, TrieMask, TrieNode, EMPTY_ROOT_HASH,
    };

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
            Nibbles::from_nibbles_unchecked(key.to_vec()),
            encode_account_value(value_nonce),
        ))
    }

    fn create_extension_node(key: &[u8], child_hash: B256) -> TrieNode {
        TrieNode::Extension(ExtensionNode::new(
            Nibbles::from_nibbles_unchecked(key.to_vec()),
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
            let idx = path_subtrie_index_unchecked(&child_path);
            let lower_subtrie = trie.lower_subtries[idx].as_ref().unwrap();
            assert_eq!(
                lower_subtrie.nodes.get(child_path),
                Some(&SparseNode::Hash(child_hashes[i])),
            );
        }
    }
}
