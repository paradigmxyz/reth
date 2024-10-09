use alloy_primitives::{
    hex, keccak256,
    map::{hash_map, HashMap},
    Bytes, B256,
};
use alloy_rlp::Decodable;
use reth_trie::prefix_set::{PrefixSet, PrefixSetMut};
use reth_trie_common::{
    word_rlp, BranchNode, BranchNodeRef, ExtensionNode, ExtensionNodeRef, LeafNode, LeafNodeRef,
    Nibbles, TrieMask, TrieNode, CHILD_INDEX_RANGE, EMPTY_ROOT_HASH,
};
use smallvec::SmallVec;
use std::{
    collections::{HashSet, VecDeque},
    fmt,
    sync::mpsc::{self, SyncSender},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SparseTrieError {}

type LeafValue = Vec<u8>;

/// Inner representation of the sparse trie.
/// Sparse trie is blind by default until nodes are revealed.
#[derive(PartialEq, Eq, Default, Debug)]
pub enum SparseTrie {
    /// None of the trie nodes are known.
    #[default]
    Blind,
    /// The trie nodes have been revealed.
    Revealed(RevealedSparseTrie),
}

impl SparseTrie {
    /// Creates new revealed empty trie.
    pub fn revealed_empty() -> Self {
        Self::Revealed(RevealedSparseTrie::default())
    }

    /// Returns `true` if the sparse trie has no revealed nodes.
    pub fn is_blind(&self) -> bool {
        matches!(self, Self::Blind)
    }

    /// Returns mutable reference to revealed sparse trie if the trie is not blind.
    pub fn as_revealed_mut(&mut self) -> Option<&mut RevealedSparseTrie> {
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
    pub fn reveal_root(&mut self, root: TrieNode) -> alloy_rlp::Result<&mut RevealedSparseTrie> {
        if self.is_blind() {
            *self = Self::Revealed(RevealedSparseTrie::from_root(root)?)
        }
        Ok(self.as_revealed_mut().unwrap())
    }

    /// Calculates and returns the trie root if the trie has been revealed.
    pub fn root(&mut self) -> Option<B256> {
        Some(self.as_revealed_mut()?.root())
    }
}

/// The representation of revealed sparse trie.
#[derive(PartialEq, Eq)]
pub struct RevealedSparseTrie {
    /// All trie nodes.
    nodes: HashMap<Nibbles, SparseNode>,
    /// All leaf values.
    values: HashMap<Nibbles, LeafValue>,
    /// Prefix set.
    prefix_set: PrefixSetMut,
    /// Re-usable buffer for RLP encoding of nodes.
    rlp_buf: Vec<u8>,
}

impl fmt::Debug for RevealedSparseTrie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevealedSparseTrie")
            .field("nodes", &self.nodes)
            .field("values", &self.values)
            .field("prefix_set", &self.prefix_set)
            .field("rlp_buf", &hex::encode(&self.rlp_buf))
            .finish()
    }
}

impl Default for RevealedSparseTrie {
    fn default() -> Self {
        Self {
            nodes: HashMap::from_iter([(Nibbles::default(), SparseNode::Empty)]),
            values: HashMap::default(),
            prefix_set: PrefixSetMut::default(),
            rlp_buf: Vec::new(),
        }
    }
}

impl RevealedSparseTrie {
    /// Create new revealed sparse trie from the given root node.
    pub fn from_root(node: TrieNode) -> alloy_rlp::Result<Self> {
        let mut this = Self {
            nodes: HashMap::default(),
            values: HashMap::default(),
            prefix_set: PrefixSetMut::default(),
            rlp_buf: Vec::new(),
        };
        this.reveal_node(Nibbles::default(), node)?;
        Ok(this)
    }

    /// Reveal the trie node only if it was not known already.
    pub fn reveal_node(&mut self, path: Nibbles, node: TrieNode) -> alloy_rlp::Result<()> {
        // TODO: revise all inserts to not overwrite existing entries
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
                self.nodes
                    .insert(path, SparseNode::Branch { state_mask: branch.state_mask, hash: None });
            }
            TrieNode::Extension(ext) => {
                let mut child_path = path.clone();
                child_path.extend_from_slice_unchecked(&ext.key);
                self.reveal_node_or_hash(child_path, &ext.child)?;
                self.nodes.insert(path, SparseNode::Extension { key: ext.key, hash: None });
            }
            TrieNode::Leaf(leaf) => {
                let mut full = path.clone();
                full.extend_from_slice_unchecked(&leaf.key);
                self.values.insert(full, leaf.value);
                self.nodes.insert(path, SparseNode::Leaf { key: leaf.key, hash: None });
            }
        }

        Ok(())
    }

    fn reveal_node_or_hash(&mut self, path: Nibbles, child: &[u8]) -> alloy_rlp::Result<()> {
        if child.len() == B256::len_bytes() + 1 {
            // TODO: revise insert to not overwrite existing entries
            self.nodes.insert(path, SparseNode::Hash(B256::from_slice(&child[1..])));
            return Ok(())
        }

        self.reveal_node(path, TrieNode::decode(&mut &child[..])?)
    }

    /// Update the leaf node with provided value.
    pub fn update_leaf(&mut self, path: Nibbles, value: LeafValue) {
        self.prefix_set.insert(path.clone());
        let existing = self.values.insert(path.clone(), value);
        if existing.is_some() {
            // trie structure unchanged, return immediately
            return
        }

        let mut current = Nibbles::default();
        while let Some(node) = self.nodes.get_mut(&current) {
            match node {
                SparseNode::Empty => {
                    *node = SparseNode::new_leaf(path);
                    break
                }
                SparseNode::Hash(_) => {
                    unimplemented!() // TODO: error out
                }
                SparseNode::Leaf { key: current_key, .. } => {
                    current.extend_from_slice_unchecked(&current_key);

                    // this leaf is being updated
                    if current == path {
                        // TODO: unreachable
                        break
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
    }

    /// Remove leaf node from the trie.
    pub fn remove_leaf(&mut self, _path: Nibbles) {
        unimplemented!()

        // let mut stack = Vec::new();
        // let mut current = Nibbles::default();
        // while let Some(node) = self.nodes.get(&current) {
        //     stack.push(current.clone());
        //     match node {
        //         SparseNode::Empty => return,
        //         SparseNode::Hash(_) => {
        //             unimplemented!() // TODO: error out
        //         }
        //         SparseNode::Leaf { key, .. } => {
        //             current.extend_from_slice_unchecked(&key);
        //             if current != path {
        //                 return
        //             }

        //             stack.push(current);
        //             break
        //         }
        //         SparseNode::Extension { key, .. } => {
        //             current.extend_from_slice_unchecked(&key);
        //             if !path.starts_with(&current) {
        //                 return
        //             }
        //         }
        //         SparseNode::Branch { state_mask, .. } => {
        //             let nibble = path[current.len()];
        //             current.push_unchecked(nibble);
        //             if !state_mask.is_bit_set(nibble) {
        //                 return
        //             }
        //         }
        //     }
        // }

        // we removed the leaf key
        // let mut child_removed = false;
        // while let hash_map::()

        // while let hash_map::Entry::Occupied(entry) = self.nodes.entry(&current) {
        //     match entry.get() {
        //         SparseNode::Empty => {} // nothing to remove
        //         SparseNode::Hash(_) => {
        //             unimplemented!() // TODO: error out
        //         }
        //         SparseNode::Leaf { key, value } => {
        //             current.extend_from_slice_unchecked(&key);

        //             if current == path {
        //                 // TODO: strip current
        //                 entry.remove();
        //                 continue
        //             }
        //         }
        //         _ => unimplemented!(),
        //     }
        // }
    }

    /// Return the root of the sparse trie.
    /// Updates all remaining dirty nodes before calculating the root.
    pub fn root(&mut self) -> B256 {
        // take the current prefix set.
        let mut prefix_set = std::mem::take(&mut self.prefix_set).freeze();
        let root_rlp = self.rlp_node(Nibbles::default(), &mut prefix_set);
        if root_rlp.len() == B256::len_bytes() + 1 {
            B256::from_slice(&root_rlp[1..])
        } else {
            keccak256(root_rlp)
        }
    }

    /// Update node hashes only if their path exceeds the provided level.
    pub fn update_rlp_node_level(&mut self, min_len: usize) {
        let mut paths = Vec::from([Nibbles::default()]);
        let mut targets = HashSet::<Nibbles>::default();

        while let Some(mut path) = paths.pop() {
            match self.nodes.get(&path).unwrap() {
                SparseNode::Empty | SparseNode::Hash(_) => {}
                SparseNode::Leaf { key, .. } => {
                    targets.insert(path);
                }
                SparseNode::Extension { key, hash } => {
                    if path.len() >= min_len {
                        targets.insert(path);
                    } else {
                        path.extend_from_slice_unchecked(key);
                        paths.push(path);
                    }
                }
                SparseNode::Branch { state_mask, hash } => {
                    if path.len() >= min_len {
                        targets.insert(path);
                    } else {
                        for bit in CHILD_INDEX_RANGE {
                            if state_mask.is_bit_set(bit) {
                                let mut child_path = path.clone();
                                child_path.push_unchecked(bit);
                                paths.push(child_path);
                            }
                        }
                    }
                }
            }
        }

        let mut prefix_set = self.prefix_set.clone().freeze();
        for target in targets {
            self.rlp_node(target, &mut prefix_set);
        }
    }

    fn rlp_node(&mut self, path: Nibbles, prefix_set: &mut PrefixSet) -> Vec<u8> {
        // stack of paths we need rlp nodes for
        let mut path_stack = Vec::from([path.clone()]);
        // stack of rlp nodes
        let mut rlp_node_stack = Vec::<(Nibbles, Vec<u8>)>::new();
        // reusable branch child path
        let mut branch_child_buf = SmallVec::<[Nibbles; 16]>::new_const();
        // reusable branch value stack
        let mut branch_value_stack_buf = SmallVec::<[Vec<u8>; 16]>::new_const();

        'main: while let Some(path) = path_stack.pop() {
            let rlp_node = match self.nodes.get_mut(&path).unwrap() {
                SparseNode::Empty => word_rlp(&EMPTY_ROOT_HASH),
                SparseNode::Hash(hash) => word_rlp(&hash),
                SparseNode::Leaf { key, hash } => {
                    self.rlp_buf.clear();
                    let mut path = path.clone();
                    path.extend_from_slice_unchecked(&key);
                    if let Some(hash) = hash.filter(|_| !prefix_set.contains(&path)) {
                        word_rlp(&hash)
                    } else {
                        let value = self.values.get(&path).unwrap();
                        let rlp_node = LeafNodeRef { key, value }.rlp(&mut self.rlp_buf);
                        if rlp_node.len() == B256::len_bytes() + 1 {
                            *hash = Some(B256::from_slice(&rlp_node[1..]));
                        }
                        rlp_node
                    }
                }
                SparseNode::Extension { key, hash } => {
                    let mut child_path = path.clone();
                    child_path.extend_from_slice_unchecked(&key);
                    if let Some(hash) = hash.filter(|_| !prefix_set.contains(&path)) {
                        word_rlp(&hash)
                    } else if rlp_node_stack.last().map_or(false, |e| e.0 == child_path) {
                        let (_, child) = rlp_node_stack.pop().unwrap();
                        self.rlp_buf.clear();
                        let rlp_node = ExtensionNodeRef::new(key, &child).rlp(&mut self.rlp_buf);
                        if rlp_node.len() == B256::len_bytes() + 1 {
                            *hash = Some(B256::from_slice(&rlp_node[1..]));
                        }
                        rlp_node
                    } else {
                        path_stack.extend([path, child_path]); // need to get rlp node for child first
                        continue
                    }
                }
                SparseNode::Branch { state_mask, hash } => {
                    if let Some(hash) = hash.filter(|_| !prefix_set.contains(&path)) {
                        rlp_node_stack.push((path, word_rlp(&hash)));
                        continue
                    }

                    branch_child_buf.clear();
                    for bit in CHILD_INDEX_RANGE {
                        if state_mask.is_bit_set(bit) {
                            let mut child = path.clone();
                            child.push_unchecked(bit);
                            branch_child_buf.push(child);
                        }
                    }

                    branch_value_stack_buf.clear();
                    for child_path in &branch_child_buf {
                        if rlp_node_stack.last().map_or(false, |e| &e.0 == child_path) {
                            let (_, child) = rlp_node_stack.pop().unwrap();
                            branch_value_stack_buf.push(child);
                        } else {
                            debug_assert!(branch_value_stack_buf.is_empty());
                            path_stack.push(path);
                            path_stack.extend(branch_child_buf.drain(..));
                            continue 'main
                        }
                    }

                    self.rlp_buf.clear();
                    let rlp_node = BranchNodeRef::new(&branch_value_stack_buf, &state_mask)
                        .rlp(&mut self.rlp_buf);
                    if rlp_node.len() == B256::len_bytes() + 1 {
                        *hash = Some(B256::from_slice(&rlp_node[1..]));
                    }
                    rlp_node
                }
            };
            rlp_node_stack.push((path, rlp_node));
        }

        rlp_node_stack.pop().unwrap().1
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum SparseNode {
    /// Empty trie node.
    Empty,
    /// The hash of the node that was not revealed.
    Hash(B256),
    /// Sparse leaf node with remaining key suffix.
    Leaf { key: Nibbles, hash: Option<B256> },
    /// Sparse extension node with key.
    Extension { key: Nibbles, hash: Option<B256> },
    /// Sparse branch node with state mask.
    Branch { state_mask: TrieMask, hash: Option<B256> },
}

impl SparseNode {
    /// Create new sparse node from [`TrieNode`].
    pub fn from_node(node: TrieNode) -> Self {
        match node {
            TrieNode::EmptyRoot => Self::Empty,
            TrieNode::Leaf(leaf) => Self::new_leaf(leaf.key),
            TrieNode::Extension(ext) => Self::new_ext(ext.key),
            TrieNode::Branch(branch) => Self::Branch { state_mask: branch.state_mask, hash: None },
        }
    }

    /// Create new [`SparseNode::Branch`] with two bits set.
    pub fn new_split_branch(bit_a: u8, bit_b: u8) -> Self {
        let state_mask = TrieMask::new(
            // set bits for both children
            (1u16 << bit_a) | (1u16 << bit_b),
        );
        Self::Branch { state_mask, hash: None }
    }

    /// Create new [`SparseNode::Extension`] from the key slice.
    pub fn new_ext(key: Nibbles) -> Self {
        Self::Extension { key, hash: None }
    }

    /// Create new [`SparseNode::Leaf`] from leaf key and value.
    pub fn new_leaf(key: Nibbles) -> Self {
        Self::Leaf { key, hash: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SparseStateTrie;
    use alloy_primitives::U256;
    use itertools::Itertools;
    use proptest::prelude::*;
    use reth_trie_common::{proof::ProofRetainer, HashBuilder};

    #[test]
    fn sparse_trie_is_blind() {
        assert!(SparseTrie::default().is_blind());
        assert!(!SparseTrie::revealed_empty().is_blind());
    }

    #[test]
    fn sparse_trie_reveal_empty() {
        let retainer = ProofRetainer::from_iter([Nibbles::default()]);
        let mut hash_builder = HashBuilder::default().with_proof_retainer(retainer);
        hash_builder.root();
        let proofs = hash_builder.take_proof_nodes();
        assert_eq!(proofs.len(), 1);

        let mut sparse = SparseStateTrie::default();
        assert_eq!(sparse.state, SparseTrie::Blind);
        sparse.reveal(Nibbles::default(), proofs.into_inner()).unwrap();
        assert_eq!(sparse.state, SparseTrie::revealed_empty());
    }

    #[test]
    fn sparse_trie_empty_update_one() {
        let path = Nibbles::unpack(B256::with_last_byte(42));
        let value = alloy_rlp::encode_fixed_size(&U256::from(1));

        let mut hash_builder = HashBuilder::default();
        hash_builder.add_leaf(path.clone(), &value);
        let expected = hash_builder.root();

        let mut sparse = SparseStateTrie::from_state(SparseTrie::revealed_empty());
        sparse.update_leaf(path, value.to_vec());
        let root = sparse.root().unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn sparse_trie_empty_update_multiple_lower_nibbles() {
        let paths = (0..=16).map(|b| Nibbles::unpack(B256::with_last_byte(b))).collect::<Vec<_>>();
        let value = alloy_rlp::encode_fixed_size(&U256::from(1));

        let mut hash_builder = HashBuilder::default();
        for path in &paths {
            hash_builder.add_leaf(path.clone(), &value);
        }
        let expected = hash_builder.root();

        let mut sparse = SparseStateTrie::from_state(SparseTrie::revealed_empty());
        for path in &paths {
            sparse.update_leaf(path.clone(), value.to_vec());
        }
        let root = sparse.root().unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn sparse_trie_empty_update_multiple_upper_nibbles() {
        let paths = (239..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let value = alloy_rlp::encode_fixed_size(&U256::from(1));

        let mut hash_builder = HashBuilder::default();
        for path in &paths {
            hash_builder.add_leaf(path.clone(), &value);
        }
        let expected = hash_builder.root();

        let mut sparse = SparseStateTrie::from_state(SparseTrie::revealed_empty());
        for path in &paths {
            sparse.update_leaf(path.clone(), value.to_vec());
        }
        let root = sparse.root().unwrap();
        assert_eq!(root, expected);
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
        let value = alloy_rlp::encode_fixed_size(&U256::from(1));

        let mut hash_builder = HashBuilder::default();
        for path in paths.iter().sorted_unstable_by_key(|key| *key) {
            hash_builder.add_leaf(path.clone(), &value);
        }
        let expected = hash_builder.root();

        let mut sparse = SparseStateTrie::from_state(SparseTrie::revealed_empty());
        for path in &paths {
            sparse.update_leaf(path.clone(), value.to_vec());
        }
        let root = sparse.root().unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn sparse_trie_empty_update_repeated() {
        let paths = (0..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let old_value = alloy_rlp::encode_fixed_size(&U256::from(1));
        let new_value = alloy_rlp::encode_fixed_size(&U256::from(2));

        let mut hash_builder = HashBuilder::default();
        for path in paths.iter().sorted_unstable_by_key(|key| *key) {
            hash_builder.add_leaf(path.clone(), &old_value);
        }
        let expected = hash_builder.root();

        let mut sparse = SparseStateTrie::from_state(SparseTrie::revealed_empty());
        for path in &paths {
            sparse.update_leaf(path.clone(), old_value.to_vec());
        }
        let root = sparse.root().unwrap();
        assert_eq!(root, expected);

        let mut hash_builder = HashBuilder::default();
        for path in paths.iter().sorted_unstable_by_key(|key| *key) {
            hash_builder.add_leaf(path.clone(), &new_value);
        }
        let expected = hash_builder.root();

        for path in &paths {
            sparse.update_leaf(path.clone(), new_value.to_vec());
        }
        let root = sparse.root().unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn sparse_trie_empty_update_fuzz() {
        proptest!(ProptestConfig::with_cases(10), |(updates: Vec<HashMap<B256, U256>>)| {
            let mut state = std::collections::BTreeMap::default();
            let mut sparse = SparseStateTrie::from_state(SparseTrie::revealed_empty());

            for update in updates {
                for (key, value) in &update {
                    sparse.update_leaf(Nibbles::unpack(key), alloy_rlp::encode_fixed_size(value).to_vec());
                }
                let root = sparse.root().unwrap();

                state.extend(update);
                let mut hash_builder = HashBuilder::default();
                for (key, value) in &state {
                    hash_builder.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode_fixed_size(value));
                }
                let expected = hash_builder.root();

                assert_eq!(root, expected);
            }
        });
    }

    #[cfg(debug_assertions)]
    mod debug_assertions {
        use super::*;
        use alloy_rlp::EMPTY_STRING_CODE;

        #[test]
        #[should_panic]
        fn reveal_first_node_not_root() {
            let mut sparse = SparseStateTrie::default();
            let proof = [(Nibbles::from_nibbles(&[0x1]), Bytes::from([EMPTY_STRING_CODE]))];
            sparse.reveal(Default::default(), proof).unwrap();
        }

        #[test]
        #[should_panic]
        fn reveal_invalid_proof_with_empty_root() {
            let mut sparse = SparseStateTrie::default();
            let proof = [
                (Nibbles::default(), Bytes::from([EMPTY_STRING_CODE])),
                (Nibbles::from_nibbles(&[0x1]), Bytes::new()),
            ];
            sparse.reveal(Default::default(), proof).unwrap();
        }
    }
}
