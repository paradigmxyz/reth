use crate::{
    nodes::{rlp_hash, BranchNode, ExtensionNode, LeafNode},
    Nibbles,
};
use reth_primitives::{
    keccak256,
    proofs::EMPTY_ROOT,
    trie::{BranchNodeCompact, TrieMask},
    H256,
};
use std::{fmt::Debug, sync::mpsc};

mod value;
use value::HashBuilderValue;

/// A type alias for a sender of branch nodes.
/// Branch nodes are sent by the Hash Builder to be stored in the database.
pub type BranchNodeSender = mpsc::Sender<(Nibbles, BranchNodeCompact)>;

/// A component used to construct the root hash of the trie. The primary purpose of a Hash Builder
/// is to build the Merkle proof that is essential for verifying the integrity and authenticity of
/// the trie's contents. It achieves this by constructing the root hash from the hashes of child
/// nodes according to specific rules, depending on the type of the node (branch, extension, or
/// leaf).
///
/// Here's an overview of how the Hash Builder works for each type of node:
///  * Branch Node: The Hash Builder combines the hashes of all the child nodes of the branch node,
///    using a cryptographic hash function like SHA-256. The child nodes' hashes are concatenated
///    and hashed, and the result is considered the hash of the branch node. The process is repeated
///    recursively until the root hash is obtained.
///  * Extension Node: In the case of an extension node, the Hash Builder first encodes the node's
///    shared nibble path, followed by the hash of the next child node. It concatenates these values
///    and then computes the hash of the resulting data, which represents the hash of the extension
///    node.
///  * Leaf Node: For a leaf node, the Hash Builder first encodes the key-path and the value of the
///    leaf node. It then concatenates the encoded key-path and value, and computes the hash of this
///    concatenated data, which represents the hash of the leaf node.
///
/// The Hash Builder operates recursively, starting from the bottom of the trie and working its way
/// up, combining the hashes of child nodes and ultimately generating the root hash. The root hash
/// can then be used to verify the integrity and authenticity of the trie's data by constructing and
/// verifying Merkle proofs.
#[derive(Clone, Debug, Default)]
pub struct HashBuilder {
    key: Nibbles,
    stack: Vec<Vec<u8>>,
    value: HashBuilderValue,

    groups: Vec<TrieMask>,
    tree_masks: Vec<TrieMask>,
    hash_masks: Vec<TrieMask>,

    stored_in_database: bool,

    branch_node_sender: Option<BranchNodeSender>,
}

impl HashBuilder {
    /// Creates a new instance of the Hash Builder.
    pub fn new(store_tx: Option<BranchNodeSender>) -> Self {
        Self { branch_node_sender: store_tx, ..Default::default() }
    }

    /// Set a branch node sender on the Hash Builder instance.
    pub fn with_branch_node_sender(mut self, tx: BranchNodeSender) -> Self {
        self.branch_node_sender = Some(tx);
        self
    }

    /// Print the current stack of the Hash Builder.
    pub fn print_stack(&self) {
        println!("============ STACK ===============");
        for item in &self.stack {
            println!("{}", hex::encode(item));
        }
        println!("============ END STACK ===============");
    }

    /// Adds a new leaf element & its value to the trie hash builder.
    pub fn add_leaf(&mut self, key: Nibbles, value: &[u8]) {
        assert!(key > self.key);
        if !self.key.is_empty() {
            self.update(&key);
        }
        self.set_key_value(key, value);
    }

    /// Adds a new branch element & its hash to the trie hash builder.
    pub fn add_branch(&mut self, key: Nibbles, value: H256, stored_in_database: bool) {
        assert!(key > self.key || (self.key.is_empty() && key.is_empty()));
        if !self.key.is_empty() {
            self.update(&key);
        } else if key.is_empty() {
            self.stack.push(rlp_hash(value));
        }
        self.set_key_value(key, value);
        self.stored_in_database = stored_in_database;
    }

    fn set_key_value<T: Into<HashBuilderValue>>(&mut self, key: Nibbles, value: T) {
        tracing::trace!(target: "trie::hash_builder", key = ?self.key, value = ?self.value, "old key/value");
        self.key = key;
        self.value = value.into();
        tracing::trace!(target: "trie::hash_builder", key = ?self.key, value = ?self.value, "new key/value");
    }

    /// Returns the current root hash of the trie builder.
    pub fn root(&mut self) -> H256 {
        // Clears the internal state
        if !self.key.is_empty() {
            self.update(&Nibbles::default());
            self.key.clear();
            self.value = HashBuilderValue::Bytes(vec![]);
        }
        self.current_root()
    }

    fn current_root(&self) -> H256 {
        if let Some(node_ref) = self.stack.last() {
            if node_ref.len() == H256::len_bytes() + 1 {
                H256::from_slice(&node_ref[1..])
            } else {
                keccak256(node_ref)
            }
        } else {
            EMPTY_ROOT
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

        tracing::debug!(target: "trie::hash_builder", ?current, ?succeeding, "updating merkle tree");

        let mut i = 0;
        loop {
            let span = tracing::span!(
                target: "trie::hash_builder",
                tracing::Level::TRACE,
                "loop",
                i,
                current = hex::encode(&current.hex_data),
                ?build_extensions
            );
            let _enter = span.enter();

            let preceding_exists = !self.groups.is_empty();
            let preceding_len: usize = self.groups.len().saturating_sub(1);

            let common_prefix_len = succeeding.common_prefix_length(&current);
            let len = std::cmp::max(preceding_len, common_prefix_len);
            assert!(len < current.len());

            tracing::trace!(
                target: "trie::hash_builder",
                ?len,
                ?common_prefix_len,
                ?preceding_len,
                preceding_exists,
                "prefix lengths after comparing keys"
            );

            // Adjust the state masks for branch calculation
            let extra_digit = current[len];
            if self.groups.len() <= len {
                let new_len = len + 1;
                tracing::trace!(target: "trie::hash_builder", new_len, old_len = self.groups.len(), "scaling state masks to fit");
                self.groups.resize(new_len, TrieMask::default());
            }
            self.groups[len] |= TrieMask::from_nibble(extra_digit);
            tracing::trace!(
                target: "trie::hash_builder",
                ?extra_digit,
                groups = self.groups.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(","),
            );

            // Adjust the tree masks for exporting to the DB
            if self.tree_masks.len() < current.len() {
                self.resize_masks(current.len());
            }

            let mut len_from = len;
            if !succeeding.is_empty() || preceding_exists {
                len_from += 1;
            }
            tracing::trace!(target: "trie::hash_builder", "skipping {} nibbles", len_from);

            // The key without the common prefix
            let short_node_key = current.slice_from(len_from);
            tracing::trace!(target: "trie::hash_builder", ?short_node_key);

            // Concatenate the 2 nodes together
            if !build_extensions {
                match &self.value {
                    HashBuilderValue::Bytes(leaf_value) => {
                        let leaf_node = LeafNode::new(&short_node_key, leaf_value);
                        tracing::debug!(target: "trie::hash_builder", ?leaf_node, "pushing leaf node");
                        tracing::trace!(target: "trie::hash_builder", rlp = hex::encode(&leaf_node.rlp()), "leaf node rlp");
                        self.stack.push(leaf_node.rlp());
                    }
                    HashBuilderValue::Hash(hash) => {
                        tracing::debug!(target: "trie::hash_builder", ?hash, "pushing branch node hash");
                        self.stack.push(rlp_hash(*hash));

                        if self.stored_in_database {
                            self.tree_masks[current.len() - 1] |=
                                TrieMask::from_nibble(current.last().unwrap());
                        }
                        self.hash_masks[current.len() - 1] |=
                            TrieMask::from_nibble(current.last().unwrap());

                        build_extensions = true;
                    }
                }
            }

            if build_extensions && !short_node_key.is_empty() {
                self.update_masks(&current, len_from);
                let stack_last =
                    self.stack.pop().expect("there should be at least one stack item; qed");
                let extension_node = ExtensionNode::new(&short_node_key, &stack_last);
                tracing::debug!(target: "trie::hash_builder", ?extension_node, "pushing extension node");
                tracing::trace!(target: "trie::hash_builder", rlp = hex::encode(&extension_node.rlp()), "extension node rlp");
                self.stack.push(extension_node.rlp());
                self.resize_masks(len_from);
            }

            if preceding_len <= common_prefix_len && !succeeding.is_empty() {
                tracing::trace!(target: "trie::hash_builder", "no common prefix to create branch nodes from, returning");
                return
            }

            // Insert branch nodes in the stack
            if !succeeding.is_empty() || preceding_exists {
                // Pushes the corresponding branch node to the stack
                let children = self.push_branch_node(len);
                // Need to store the branch node in an efficient format
                // outside of the hash builder
                self.store_branch_node(&current, len, children);
            }

            self.groups.resize(len, TrieMask::default());
            self.resize_masks(len);

            if preceding_len == 0 {
                tracing::trace!(target: "trie::hash_builder", "0 or 1 state masks means we have no more elements to process");
                return
            }

            current.truncate(preceding_len);
            tracing::trace!(target: "trie::hash_builder", ?current, "truncated nibbles to {} bytes", preceding_len);

            tracing::trace!(target: "trie::hash_builder", groups = ?self.groups, "popping empty state masks");
            while self.groups.last() == Some(&TrieMask::default()) {
                self.groups.pop();
            }

            build_extensions = true;

            i += 1;
        }
    }

    /// Given the size of the longest common prefix, it proceeds to create a branch node
    /// from the state mask and existing stack state, and store its RLP to the top of the stack,
    /// after popping all the relevant elements from the stack.
    fn push_branch_node(&mut self, len: usize) -> Vec<H256> {
        let state_mask = self.groups[len];
        let hash_mask = self.hash_masks[len];
        let branch_node = BranchNode::new(&self.stack);
        let children = branch_node.children(state_mask, hash_mask).collect();
        let rlp = branch_node.rlp(state_mask);

        // Clears the stack from the branch node elements
        let first_child_idx = self.stack.len() - state_mask.count_ones() as usize;
        tracing::debug!(
            target: "trie::hash_builder",
            new_len = first_child_idx,
            old_len = self.stack.len(),
            "resizing stack to prepare branch node"
        );
        self.stack.resize(first_child_idx, vec![]);

        tracing::debug!(target: "trie::hash_builder", "pushing branch node with {:?} mask from stack", state_mask);
        tracing::trace!(target: "trie::hash_builder", rlp = hex::encode(&rlp), "branch node rlp");
        self.stack.push(rlp);
        children
    }

    /// Given the current nibble prefix and the highest common prefix length, proceeds
    /// to update the masks for the next level and store the branch node and the
    /// masks in the database. We will use that when consuming the intermediate nodes
    /// from the database to efficiently build the trie.
    fn store_branch_node(&mut self, current: &Nibbles, len: usize, children: Vec<H256>) {
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

            let mut n = BranchNodeCompact::new(
                self.groups[len],
                self.tree_masks[len],
                self.hash_masks[len],
                children,
                None,
            );

            if len == 0 {
                n.root_hash = Some(self.current_root());
            }

            // Send it over to the provided channel which will handle it on the
            // other side of the HashBuilder
            tracing::debug!(target: "trie::hash_builder", node = ?n, "intermediate node");
            let common_prefix = current.slice(0, len);
            if let Some(tx) = &self.branch_node_sender {
                let _ = tx.send((common_prefix, n));
            }
        }
    }

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
        tracing::trace!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use reth_primitives::{hex_literal::hex, proofs::KeccakHasher, H256, U256};
    use std::collections::{BTreeMap, HashMap};

    fn trie_root<I, K, V>(iter: I) -> H256
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<[u8]> + Ord,
        V: AsRef<[u8]>,
    {
        // We use `trie_root` instead of `sec_trie_root` because we assume
        // the incoming keys are already hashed, which makes sense given
        // we're going to be using the Hashed tables & pre-hash the data
        // on the way in.
        triehash::trie_root::<KeccakHasher, _, _, _>(iter)
    }

    // Hashes the keys, RLP encodes the values, compares the trie builder with the upstream root.
    fn assert_hashed_trie_root<'a, I, K>(iter: I)
    where
        I: Iterator<Item = (K, &'a U256)>,
        K: AsRef<[u8]> + Ord,
    {
        let hashed = iter
            .map(|(k, v)| (keccak256(k.as_ref()), reth_rlp::encode_fixed_size(v).to_vec()))
            // Collect into a btree map to sort the data
            .collect::<BTreeMap<_, _>>();

        let mut hb = HashBuilder::default();

        hashed.iter().for_each(|(key, val)| {
            let nibbles = Nibbles::unpack(key);
            hb.add_leaf(nibbles, val);
        });

        assert_eq!(hb.root(), trie_root(&hashed));
    }

    // No hashing involved
    fn assert_trie_root<I, K, V>(iter: I)
    where
        I: Iterator<Item = (K, V)>,
        K: AsRef<[u8]> + Ord,
        V: AsRef<[u8]>,
    {
        let mut hb = HashBuilder::default();

        let data = iter.collect::<BTreeMap<_, _>>();
        data.iter().for_each(|(key, val)| {
            let nibbles = Nibbles::unpack(key);
            hb.add_leaf(nibbles, val.as_ref());
        });
        assert_eq!(hb.root(), trie_root(data));
    }

    #[test]
    fn empty() {
        assert_eq!(HashBuilder::default().root(), EMPTY_ROOT);
    }

    #[test]
    fn arbitrary_hashed_root() {
        proptest!(|(state: BTreeMap<H256, U256>)| {
            assert_hashed_trie_root(state.iter());
        });
    }

    #[test]
    fn test_generates_branch_node() {
        let (sender, recv) = mpsc::channel();
        let mut hb = HashBuilder::new(Some(sender));

        // We have 1 branch node update to be stored at 0x01, indicated by the first nibble.
        // That branch root node has 2 branch node children present at 0x1 and 0x2.
        // - 0x1 branch: It has the 2 empty items, at `0` and `1`.
        // - 0x2 branch: It has the 2 empty items, at `0` and `2`.
        // This is enough information to construct the intermediate node value:
        // 1. State Mask: 0b111. The children of the branch + the branch value at `0`, `1` and `2`.
        // 2. Hash Mask: 0b110. Of the above items, `1` and `2` correspond to sub-branch nodes.
        // 3. Tree Mask: 0b000.
        // 4. Hashes: The 2 sub-branch roots, at `1` and `2`, calculated by hashing
        // the 0th and 1st element for the 0x1 branch (according to the 3rd nibble),
        // and the 0th and 2nd element for the 0x2 branch (according to the 3rd nibble).
        // This basically means that every BranchNodeCompact is capable of storing up to 2 levels
        // deep of nodes (?).
        let data = BTreeMap::from([
            (
                hex!("1000000000000000000000000000000000000000000000000000000000000000").to_vec(),
                Vec::new(),
            ),
            (
                hex!("1100000000000000000000000000000000000000000000000000000000000000").to_vec(),
                Vec::new(),
            ),
            (
                hex!("1110000000000000000000000000000000000000000000000000000000000000").to_vec(),
                Vec::new(),
            ),
            (
                hex!("1200000000000000000000000000000000000000000000000000000000000000").to_vec(),
                Vec::new(),
            ),
            (
                hex!("1220000000000000000000000000000000000000000000000000000000000000").to_vec(),
                Vec::new(),
            ),
            (
                // unrelated leaf
                hex!("1320000000000000000000000000000000000000000000000000000000000000").to_vec(),
                Vec::new(),
            ),
        ]);
        data.iter().for_each(|(key, val)| {
            let nibbles = Nibbles::unpack(key);
            hb.add_leaf(nibbles, val.as_ref());
        });
        let root = hb.root();
        drop(hb);

        let updates = recv.iter().collect::<Vec<_>>();

        let updates = updates.iter().cloned().collect::<BTreeMap<_, _>>();
        let update = updates.get(&Nibbles::from(hex!("01").as_slice())).unwrap();
        assert_eq!(update.state_mask, TrieMask::new(0b1111)); // 1st nibble: 0, 1, 2, 3
        assert_eq!(update.tree_mask, TrieMask::new(0));
        assert_eq!(update.hash_mask, TrieMask::new(6)); // in the 1st nibble, the ones with 1 and 2 are branches with `hashes`
        assert_eq!(update.hashes.len(), 2); // calculated while the builder is running

        assert_eq!(root, trie_root(data));
    }

    #[test]
    fn test_root_raw_data() {
        let data = vec![
            (hex!("646f").to_vec(), hex!("76657262").to_vec()),
            (hex!("676f6f64").to_vec(), hex!("7075707079").to_vec()),
            (hex!("676f6b32").to_vec(), hex!("7075707079").to_vec()),
            (hex!("676f6b34").to_vec(), hex!("7075707079").to_vec()),
        ];
        assert_trie_root(data.into_iter());
    }

    #[test]
    fn test_root_rlp_hashed_data() {
        let data = HashMap::from([
            (H256::from_low_u64_le(1), U256::from(2)),
            (H256::from_low_u64_be(3), U256::from(4)),
        ]);
        assert_hashed_trie_root(data.iter());
    }

    #[test]
    fn test_root_known_hash() {
        let root_hash = H256::random();
        let mut hb = HashBuilder::default();
        hb.add_branch(Nibbles::default(), root_hash, false);
        assert_eq!(hb.root(), root_hash);
    }

    #[test]
    fn manual_branch_node_ok() {
        let raw_input = vec![
            (hex!("646f").to_vec(), hex!("76657262").to_vec()),
            (hex!("676f6f64").to_vec(), hex!("7075707079").to_vec()),
        ];
        let input =
            raw_input.iter().map(|(key, value)| (Nibbles::unpack(key), value)).collect::<Vec<_>>();

        // We create the hash builder and add the leaves
        let mut hb = HashBuilder::default();
        for (key, val) in input.iter() {
            hb.add_leaf(key.clone(), val.as_slice());
        }

        // Manually create the branch node that should be there after the first 2 leaves are added.
        // Skip the 0th element given in this example they have a common prefix and will
        // collapse to a Branch node.
        use reth_primitives::bytes::BytesMut;
        use reth_rlp::Encodable;
        let leaf1 = LeafNode::new(&Nibbles::unpack(&raw_input[0].0[1..]), input[0].1);
        let leaf2 = LeafNode::new(&Nibbles::unpack(&raw_input[1].0[1..]), input[1].1);
        let mut branch: [&dyn Encodable; 17] = [b""; 17];
        // We set this to `4` and `7` because that mathces the 2nd element of the corresponding
        // leaves. We set this to `7` because the 2nd element of Leaf 1 is `7`.
        branch[4] = &leaf1;
        branch[7] = &leaf2;
        let mut branch_node_rlp = BytesMut::new();
        reth_rlp::encode_list::<dyn Encodable, _>(&branch, &mut branch_node_rlp);
        let branch_node_hash = keccak256(branch_node_rlp);

        let mut hb2 = HashBuilder::default();
        // Insert the branch with the `0x6` shared prefix.
        hb2.add_branch(Nibbles::from_hex(vec![0x6]), branch_node_hash, false);

        let expected = trie_root(raw_input.clone());
        assert_eq!(hb.root(), expected);
        assert_eq!(hb2.root(), expected);
    }
}
