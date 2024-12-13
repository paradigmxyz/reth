use crate::{
    branch::{BranchNodeRef, CHILD_INDEX_MASK},
    leaf::HashLeaf,
    sub_tree::SubTreeRef,
};
use alloy_primitives::{map::HashMap, B256};
use alloy_trie::{
    hash_builder::{HashBuilderValue, HashBuilderValueRef},
    nodes::LeafNodeRef,
    proof::{ProofNodes, ProofRetainer},
    BranchNodeCompact, Nibbles, TrieMask,
};
use core::cmp;
use reth_scroll_primitives::poseidon::EMPTY_ROOT_HASH;
use tracing::trace;

#[derive(Debug, Default)]
#[allow(missing_docs)]
pub struct HashBuilder {
    pub key: Nibbles,
    pub value: HashBuilderValue,
    pub stack: Vec<B256>,

    // TODO(scroll): Introduce terminator / leaf masks
    pub state_masks: Vec<TrieMask>,
    pub tree_masks: Vec<TrieMask>,
    pub hash_masks: Vec<TrieMask>,

    pub stored_in_database: bool,

    pub updated_branch_nodes: Option<HashMap<Nibbles, BranchNodeCompact>>,
    pub proof_retainer: Option<ProofRetainer>,
}

impl HashBuilder {
    /// Enables the Hash Builder to store updated branch nodes.
    ///
    /// Call [`HashBuilder::split`] to get the updates to branch nodes.
    pub fn with_updates(mut self, retain_updates: bool) -> Self {
        self.set_updates(retain_updates);
        self
    }

    /// Enable specified proof retainer.
    pub fn with_proof_retainer(mut self, retainer: ProofRetainer) -> Self {
        self.proof_retainer = Some(retainer);
        self
    }

    /// Enables the Hash Builder to store updated branch nodes.
    ///
    /// Call [`HashBuilder::split`] to get the updates to branch nodes.
    pub fn set_updates(&mut self, retain_updates: bool) {
        if retain_updates {
            self.updated_branch_nodes = Some(HashMap::default());
        }
    }

    /// Splits the [`HashBuilder`] into a [`HashBuilder`] and hash builder updates.
    pub fn split(mut self) -> (Self, HashMap<Nibbles, BranchNodeCompact>) {
        let updates = self.updated_branch_nodes.take();
        (self, updates.unwrap_or_default())
    }

    /// Take and return retained proof nodes.
    pub fn take_proof_nodes(&mut self) -> ProofNodes {
        self.proof_retainer.take().map(ProofRetainer::into_proof_nodes).unwrap_or_default()
    }

    /// The number of total updates accrued.
    /// Returns `0` if [`Self::with_updates`] was not called.
    pub fn updates_len(&self) -> usize {
        self.updated_branch_nodes.as_ref().map(|u| u.len()).unwrap_or(0)
    }

    /// Print the current stack of the Hash Builder.
    pub fn print_stack(&self) {
        println!("============ STACK ===============");
        for item in &self.stack {
            println!("{}", alloy_primitives::hex::encode(item));
        }
        println!("============ END STACK ===============");
    }

    /// Adds a new leaf element and its value to the trie hash builder.
    pub fn add_leaf(&mut self, key: Nibbles, value: &[u8]) {
        assert!(key > self.key, "add_leaf key {:?} self.key {:?}", key, self.key);
        if !self.key.is_empty() {
            self.update(&key);
        }
        self.set_key_value(key, HashBuilderValueRef::Bytes(value));
    }

    /// Adds a new branch element and its hash to the trie hash builder.
    pub fn add_branch(&mut self, key: Nibbles, value: B256, stored_in_database: bool) {
        assert!(
            key > self.key || (self.key.is_empty() && key.is_empty()),
            "add_branch key {:?} self.key {:?}",
            key,
            self.key
        );
        if !self.key.is_empty() {
            self.update(&key);
        } else if key.is_empty() {
            self.stack.push(value);
        }
        self.set_key_value(key, HashBuilderValueRef::Hash(&value));
        self.stored_in_database = stored_in_database;
    }

    /// Returns the current root hash of the trie builder.
    pub fn root(&mut self) -> B256 {
        // Clears the internal state
        if !self.key.is_empty() {
            self.update(&Nibbles::default());
            self.key.clear();
            self.value.clear();
        }
        let root = self.current_root();
        if root == EMPTY_ROOT_HASH {
            if let Some(proof_retainer) = self.proof_retainer.as_mut() {
                proof_retainer.retain(&Nibbles::default(), &[])
            }
        }
        root
    }

    #[inline]
    fn set_key_value(&mut self, key: Nibbles, value: HashBuilderValueRef<'_>) {
        self.log_key_value("old value");
        self.key = key;
        self.value.set_from_ref(value);
        self.log_key_value("new value");
    }

    fn log_key_value(&self, msg: &str) {
        trace!(target: "trie::hash_builder",
            key = ?self.key,
            value = ?self.value,
            "{msg}",
        );
    }

    fn current_root(&self) -> B256 {
        if let Some(node_ref) = self.stack.last() {
            let mut root = *node_ref;
            root.reverse();
            root
        } else {
            EMPTY_ROOT_HASH
        }
    }

    /// Given a new element, it appends it to the stack and proceeds to loop through the stack state
    /// and convert the nodes it can into branch / extension nodes and hash them. This ensures
    /// that the top of the stack always contains the merkle root corresponding to the trie
    /// built so far.
    fn update(&mut self, succeeding: &Nibbles) {
        let mut build_extensions = false;
        // current / self.key is always the latest added element in the trie
        let mut current = self.key.clone();
        debug_assert!(!current.is_empty());

        trace!(target: "trie::hash_builder", ?current, ?succeeding, "updating merkle tree");

        let mut i = 0usize;
        loop {
            let _span = tracing::trace_span!(target: "trie::hash_builder", "loop", i, ?current, build_extensions).entered();

            let preceding_exists = !self.state_masks.is_empty();
            let preceding_len = self.state_masks.len().saturating_sub(1);

            let common_prefix_len = succeeding.common_prefix_length(current.as_slice());
            let len = cmp::max(preceding_len, common_prefix_len);
            assert!(len < current.len(), "len {} current.len {}", len, current.len());

            trace!(
                target: "trie::hash_builder",
                ?len,
                ?common_prefix_len,
                ?preceding_len,
                preceding_exists,
                "prefix lengths after comparing keys"
            );

            // Adjust the state masks for branch calculation
            let extra_digit = current[len];
            if self.state_masks.len() <= len {
                let new_len = len + 1;
                trace!(target: "trie::hash_builder", new_len, old_len = self.state_masks.len(), "scaling state masks to fit");
                self.state_masks.resize(new_len, TrieMask::default());
            }
            self.state_masks[len] |= TrieMask::from_nibble(extra_digit);
            trace!(
                target: "trie::hash_builder",
                ?extra_digit,
                groups = ?self.state_masks,
            );

            // Adjust the tree masks for exporting to the DB
            if self.tree_masks.len() < current.len() {
                self.resize_masks(current.len());
            }

            let mut len_from = len;
            if !succeeding.is_empty() || preceding_exists {
                len_from += 1;
            }
            trace!(target: "trie::hash_builder", "skipping {len_from} nibbles");

            // The key without the common prefix
            let short_node_key = current.slice(len_from..);
            trace!(target: "trie::hash_builder", ?short_node_key);

            // Concatenate the 2 nodes together
            if !build_extensions {
                match self.value.as_ref() {
                    HashBuilderValueRef::Bytes(leaf_value) => {
                        // TODO(scroll): Replace with terminator masks
                        // Set the terminator mask for the leaf node
                        self.state_masks[len] |= TrieMask::new(0b100 << extra_digit);
                        let leaf_node = LeafNodeRef::new(&current, leaf_value);
                        let leaf_hash = leaf_node.hash_leaf();
                        trace!(
                            target: "trie::hash_builder",
                            ?leaf_node,
                            ?leaf_hash,
                            "pushing leaf node",
                        );
                        self.stack.push(leaf_hash);
                        // self.retain_proof_from_stack(&current.slice(..len_from));
                    }
                    HashBuilderValueRef::Hash(hash) => {
                        trace!(target: "trie::hash_builder", ?hash, "pushing branch node hash");
                        self.stack.push(*hash);

                        if self.stored_in_database {
                            self.tree_masks[current.len() - 1] |= TrieMask::from_nibble(
                                current
                                    .last()
                                    .expect("must have at least a single bit in the current key"),
                            );
                        }
                        self.hash_masks[current.len() - 1] |= TrieMask::from_nibble(
                            current
                                .last()
                                .expect("must have at least a single bit in the current key"),
                        );

                        build_extensions = true;
                    }
                }
            }

            if build_extensions && !short_node_key.is_empty() {
                self.update_masks(&current, len_from);
                let stack_last = self.stack.pop().expect("there should be at least one stack item");
                let sub_tree = SubTreeRef::new(&short_node_key, &stack_last);
                let sub_tree_root = sub_tree.root();

                trace!(
                    target: "trie::hash_builder",
                    ?short_node_key,
                    ?sub_tree_root,
                    "pushing subtree root",
                );
                self.stack.push(sub_tree_root);
                // self.retain_proof_from_stack(&current.slice(..len_from));
                self.resize_masks(len_from);
            }

            if preceding_len <= common_prefix_len && !succeeding.is_empty() {
                trace!(target: "trie::hash_builder", "no common prefix to create branch nodes from, returning");
                return;
            }

            // Insert branch nodes in the stack
            if !succeeding.is_empty() || preceding_exists {
                // Pushes the corresponding branch node to the stack
                let children = self.push_branch_node(&current, len);
                // Need to store the branch node in an efficient format outside of the hash builder
                self.store_branch_node(&current, len, children);
            }

            self.state_masks.resize(len, TrieMask::default());
            self.resize_masks(len);

            if preceding_len == 0 {
                trace!(target: "trie::hash_builder", "0 or 1 state masks means we have no more elements to process");
                return;
            }

            current.truncate(preceding_len);
            trace!(target: "trie::hash_builder", ?current, "truncated nibbles to {} bytes", preceding_len);

            trace!(target: "trie::hash_builder", groups = ?self.state_masks, "popping empty state masks");
            while self.state_masks.last() == Some(&TrieMask::default()) {
                self.state_masks.pop();
            }

            build_extensions = true;

            i += 1;
        }
    }

    /// Given the size of the longest common prefix, it proceeds to create a branch node
    /// from the state mask and existing stack state, and store its RLP to the top of the stack,
    /// after popping all the relevant elements from the stack.
    ///
    /// Returns the hashes of the children of the branch node, only if `updated_branch_nodes` is
    /// enabled.
    fn push_branch_node(&mut self, _current: &Nibbles, len: usize) -> Vec<B256> {
        let state_mask = self.state_masks[len];
        let hash_mask = self.hash_masks[len];
        let branch_node = BranchNodeRef::new(&self.stack, state_mask);
        // Avoid calculating this value if it's not needed.
        let children = if self.updated_branch_nodes.is_some() {
            branch_node.child_hashes(hash_mask).collect()
        } else {
            vec![]
        };

        let branch_hash = branch_node.hash();

        // TODO: enable proof retention
        // self.retain_proof_from_stack(&current.slice(..len));

        // Clears the stack from the branch node elements
        let first_child_idx = branch_node.first_child_index();
        trace!(
            target: "trie::hash_builder",
            new_len = first_child_idx,
            old_len = self.stack.len(),
            "resizing stack to prepare branch node"
        );
        self.stack.resize_with(first_child_idx, Default::default);

        trace!(target: "trie::hash_builder", ?branch_hash, "pushing branch node with {state_mask:?} mask
        from stack");

        self.stack.push(branch_hash);
        children
    }

    /// Given the current nibble prefix and the highest common prefix length, proceeds
    /// to update the masks for the next level and store the branch node and the
    /// masks in the database. We will use that when consuming the intermediate nodes
    /// from the database to efficiently build the trie.
    fn store_branch_node(&mut self, current: &Nibbles, len: usize, children: Vec<B256>) {
        trace!(target: "trie::hash_builder", ?current, ?len, ?children, "store branch node");
        if len > 0 {
            let parent_index = len - 1;
            self.hash_masks[parent_index] |= TrieMask::from_nibble(current[parent_index]);
        }

        let store_in_db_trie = !self.tree_masks[len].is_empty() || !self.hash_masks[len].is_empty();
        if store_in_db_trie {
            if len > 0 {
                let parent_index = len - 1;
                self.tree_masks[parent_index] |= TrieMask::from_nibble(current[parent_index]);
            }

            if self.updated_branch_nodes.is_some() {
                let common_prefix = current.slice(..len);
                let node = BranchNodeCompact::new(
                    self.state_masks[len] & CHILD_INDEX_MASK,
                    self.tree_masks[len],
                    self.hash_masks[len],
                    children,
                    (len == 0).then(|| self.current_root()),
                );
                trace!(target: "trie::hash_builder", ?node, "storing updated intermediate node");
                self.updated_branch_nodes
                    .as_mut()
                    .expect("updates_branch_nodes is some")
                    .insert(common_prefix, node);
            }
        }
    }

    // TODO(scroll): Enable proof retention
    // fn retain_proof_from_stack(&mut self, prefix: &Nibbles) {
    //     if let Some(proof_retainer) = self.proof_retainer.as_mut() {
    //         proof_retainer.retain(
    //             prefix,
    //             self.stack.last().expect("there should be at least one stack item").as_ref(),
    //         );
    //     }
    // }

    fn update_masks(&mut self, current: &Nibbles, len_from: usize) {
        if len_from > 0 {
            let flag = TrieMask::from_nibble(current[len_from - 1]);

            self.hash_masks[len_from - 1] &= !flag;

            if !self.tree_masks[current.len() - 1].is_empty() {
                self.tree_masks[len_from - 1] |= flag;
            }
        }
    }

    fn resize_masks(&mut self, new_len: usize) {
        trace!(
            target: "trie::hash_builder",
            new_len,
            old_tree_mask_len = self.tree_masks.len(),
            old_hash_mask_len = self.hash_masks.len(),
            "resizing tree/hash masks"
        );
        self.tree_masks.resize(new_len, TrieMask::default());
        self.hash_masks.resize(new_len, TrieMask::default());
    }
}

// TODO(scroll): Introduce generic for the HashBuilder.
impl From<reth_trie::HashBuilder> for HashBuilder {
    fn from(hash_builder: reth_trie::HashBuilder) -> Self {
        Self {
            key: hash_builder.key,
            value: hash_builder.value,
            stack: hash_builder
                .stack
                .into_iter()
                .map(|x| x.as_slice().try_into().expect("RlpNode contains 32 byte hashes"))
                .collect(),
            state_masks: hash_builder.groups,
            tree_masks: hash_builder.tree_masks,
            hash_masks: hash_builder.hash_masks,
            stored_in_database: hash_builder.stored_in_database,
            updated_branch_nodes: hash_builder.updated_branch_nodes,
            proof_retainer: hash_builder.proof_retainer,
        }
    }
}

impl From<HashBuilder> for reth_trie::HashBuilder {
    fn from(value: HashBuilder) -> Self {
        Self {
            key: value.key,
            value: value.value,
            stack: value
                .stack
                .into_iter()
                .map(|x| {
                    reth_trie::RlpNode::from_raw(&x.0).expect("32 byte hash can be cast to RlpNode")
                })
                .collect(),
            groups: value.state_masks,
            tree_masks: value.tree_masks,
            hash_masks: value.hash_masks,
            stored_in_database: value.stored_in_database,
            updated_branch_nodes: value.updated_branch_nodes,
            proof_retainer: value.proof_retainer,
            rlp_buf: Default::default(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::collections::BTreeMap;
    use hex_literal::hex;
    use reth_scroll_primitives::poseidon::{hash_with_domain, Fr, PrimeField};
    use reth_trie::BitsCompatibility;

    #[test]
    fn test_convert_to_bit_representation() {
        let nibbles = Nibbles::unpack_bits(vec![7, 8]);
        let expected = [0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0];
        assert_eq!(nibbles.as_slice(), expected);
    }

    #[test]
    fn test_convert_to_bit_representation_truncation() {
        // 64 byte nibble
        let hex = hex!("0102030405060708090a0b0c0d0e0f0102030405060708090a0b0c0d0e0f0102030405060708090a0b0c0d0e0f0102030405060708090a0b0c0d0e0f01020304");
        assert_eq!(hex.len(), 64);
        let nibbles = Nibbles::unpack_bits(hex);
        assert_eq!(nibbles.len(), 254);
    }

    #[test]
    fn test_basic_trie() {
        // Test a basic trie consisting of three key value pairs:
        // (0, 0, 0, 0, ... , 0)
        // (0, 0, 0, 1, ... , 0)
        // (0, 0, 1, 0, ... , 0)
        // (1, 1, 1, 0, ... , 0)
        // (1, 1, 1, 1, ... , 0)
        // The branch associated with key 0xF will be collapsed into a single leaf.

        let leaf_1_key = Nibbles::from_nibbles_unchecked([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let leaf_2_key = Nibbles::from_nibbles_unchecked([
            0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let leaf_3_key = Nibbles::from_nibbles_unchecked([
            0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let leaf_4_key = Nibbles::from_nibbles_unchecked([
            1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let leaf_5_key = Nibbles::from_nibbles_unchecked([
            1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let leaf_keys = [
            leaf_1_key.clone(),
            leaf_2_key.clone(),
            leaf_3_key.clone(),
            leaf_4_key.clone(),
            leaf_5_key.clone(),
        ];

        let leaf_values = leaf_keys
            .into_iter()
            .enumerate()
            .map(|(i, key)| {
                let mut leaf_value = [0u8; 32];
                leaf_value[0] = i as u8 + 1;
                (key, leaf_value)
            })
            .collect::<BTreeMap<_, _>>();

        let leaf_hashes: BTreeMap<_, _> = leaf_values
            .iter()
            .map(|(key, value)| {
                let key_fr = Fr::from_repr_vartime(key.encode_leaf_key())
                    .expect("key is valid field element");
                let value = Fr::from_repr_vartime(*value).expect("value is a valid field element");
                let hash = hash_with_domain(&[key_fr, value], crate::LEAF_NODE_DOMAIN);
                (key.clone(), hash)
            })
            .collect();

        let mut hb = HashBuilder::default().with_updates(true);

        for (key, val) in &leaf_values {
            hb.add_leaf(key.clone(), val);
        }

        let root = hb.root();

        // node_000 -> hash(leaf_1, leaf_2) LTRT
        // node_00 -> hash(node_000, leaf_3) LBRT
        // node_0 -> hash(node_00, EMPTY) LBRT
        // node_111 -> hash(leaf_4, leaf_5) LTRT
        // node_11 -> hash(EMPTY, node_111) LTRB
        // node_1 -> hash(EMPTY, node_11) LTRB
        // root -> hash(node_0, node_1) LBRB

        let expected: B256 = {
            let node_000 = hash_with_domain(
                &[*leaf_hashes.get(&leaf_1_key).unwrap(), *leaf_hashes.get(&leaf_2_key).unwrap()],
                crate::BRANCH_NODE_LTRT_DOMAIN,
            );
            let node_00 = hash_with_domain(
                &[node_000, *leaf_hashes.get(&leaf_3_key).unwrap()],
                crate::BRANCH_NODE_LBRT_DOMAIN,
            );
            let node_0 = hash_with_domain(&[node_00, Fr::zero()], crate::BRANCH_NODE_LBRT_DOMAIN);
            let node_111 = hash_with_domain(
                &[*leaf_hashes.get(&leaf_4_key).unwrap(), *leaf_hashes.get(&leaf_5_key).unwrap()],
                crate::BRANCH_NODE_LTRT_DOMAIN,
            );
            let node_11 = hash_with_domain(&[Fr::zero(), node_111], crate::BRANCH_NODE_LTRB_DOMAIN);
            let node_1 = hash_with_domain(&[Fr::zero(), node_11], crate::BRANCH_NODE_LTRB_DOMAIN);

            let mut root =
                hash_with_domain(&[node_0, node_1], crate::BRANCH_NODE_LBRB_DOMAIN).to_repr();
            root.reverse();
            root.into()
        };

        assert_eq!(expected, root);
    }
}
