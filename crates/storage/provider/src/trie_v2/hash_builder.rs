#![allow(unused)]
use std::{default, fmt::Debug};

use reth_primitives::{keccak256, proofs::EMPTY_ROOT, H256};

use crate::trie_v2::node::{
    rlp_hash, BranchNode, BranchNodeCompact, ExtensionNode, LeafNode, KECCAK_LENGTH,
};

use super::nibbles::Nibbles;

#[derive(Clone)]
enum HashBuilderValue {
    Bytes(Vec<u8>),
    Hash(H256),
}

impl Debug for HashBuilderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(bytes) => write!(f, "Bytes({:?})", hex::encode(&bytes)),
            Self::Hash(hash) => write!(f, "Hash({:?})", hash),
        }
    }
}

impl From<Vec<u8>> for HashBuilderValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<&[u8]> for HashBuilderValue {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

impl From<H256> for HashBuilderValue {
    fn from(value: H256) -> Self {
        Self::Hash(value)
    }
}

impl Default for HashBuilderValue {
    fn default() -> Self {
        Self::Bytes(vec![])
    }
}

use std::cmp;

pub(crate) fn has_prefix(s: &[u8], prefix: &[u8]) -> bool {
    s.starts_with(prefix)
}

pub(crate) fn assert_subset(sub: u16, sup: u16) {
    assert_eq!(sub & sup, sub);
}

#[derive(Clone, Debug, Default)]
pub struct HashBuilder {
    key: Nibbles,
    stack: Vec<Vec<u8>>,
    value: HashBuilderValue,

    groups: Vec<u16>,
    tree_masks: Vec<u16>,
    hash_masks: Vec<u16>,

    is_in_db_trie: bool,
}

impl HashBuilder {
    pub fn stack_hex(&self) {
        println!("============ STACK ===============");
        for item in &self.stack {
            println!("{}", hex::encode(item));
        }
        println!("============ END STACK ===============");
    }

    pub fn new() -> Self {
        Self::default()
    }

    #[tracing::instrument(skip_all)]
    /// Adds a new leaf element & its value to the trie hash builder.
    pub fn add_leaf(&mut self, key: Nibbles, value: &[u8]) {
        assert!(key > self.key);
        self.add(key, value);
    }

    #[tracing::instrument(skip_all)]
    /// Adds a new branch element & its hash to the trie hash builder.
    pub fn add_branch(&mut self, key: Nibbles, value: H256) {
        self.add_branch_inner(key, value);
    }

    #[tracing::instrument(skip_all)]
    pub fn add_branch_from_db(&mut self, key: Nibbles, value: H256, is_in_db_trie: bool) {
        self.add_branch_inner(key, value);
        self.is_in_db_trie = is_in_db_trie;
    }

    fn add_branch_inner(&mut self, key: Nibbles, value: H256) {
        assert!(key > self.key || (self.key.is_empty() && key.is_empty()));
        if self.key.is_empty() {
            self.stack.push(rlp_hash(value));
        }
        self.add(key, value);
    }

    fn add<T: Into<HashBuilderValue>>(&mut self, key: Nibbles, value: T) {
        if !self.key.is_empty() {
            self.update(&key);
        }
        tracing::trace!(?self.key, ?self.value, "old key/value");
        self.key = key;
        self.value = value.into();
        tracing::trace!(?self.key, ?self.value, "new key/value");
    }

    /// Returns the current root hash of the trie builder.
    #[tracing::instrument(skip_all)]
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
            if node_ref.len() == KECCAK_LENGTH + 1 {
                H256::from_slice(&node_ref[1..])
            } else {
                keccak256(node_ref)
            }
        } else {
            EMPTY_ROOT
        }
    }

    #[tracing::instrument(skip_all)]
    /// Given a new element, it appends it to the stack and proceeds to loop through the stack state
    /// and convert the nodes it can into branch / extension nodes and hash them. This ensures
    /// that the top of the stack always contains the merkle root corresponding to the trie
    /// built so far.
    fn update(&mut self, succeeding: &Nibbles) {
        let mut build_extensions = false;
        // current / self.key is always the latest added element in the trie
        let mut current = self.key.clone();

        tracing::debug!(?current, ?succeeding, "updating merkle tree");

        let mut i = 0;
        loop {
            let span = tracing::span!(
                tracing::Level::TRACE,
                "loop",
                i,
                current = hex::encode(&current.hex_data),
                ?build_extensions
            );
            let _enter = span.enter();

            let preceding_exists = !self.groups.is_empty();
            let preceding_len: usize = self.groups.len().saturating_sub(1);

            let common_prefix_len = succeeding.prefix_length(&current);
            let len = std::cmp::max(preceding_len, common_prefix_len);
            assert!(len < current.len());

            tracing::trace!(
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
                tracing::trace!(new_len, old_len = self.groups.len(), "scaling state masks to fit");
                self.groups.resize(new_len, 0u16);
            }
            self.groups[len] |= 1u16 << extra_digit;
            tracing::trace!(
                ?extra_digit,
                groups =
                    self.groups.iter().map(|x| format!("{:016b}", x)).collect::<Vec<_>>().join(","),
            );

            // Adjust the tree masks for exporting to the DB
            if self.tree_masks.len() < current.len() {
                self.resize_masks(current.len());
            }

            let mut len_from = len;
            if !succeeding.is_empty() || preceding_exists {
                len_from += 1;
            }
            tracing::trace!("skipping {} nibbles", len_from);

            // The key without the common prefix
            let short_node_key = current.offset(len_from);
            tracing::trace!(?short_node_key);

            // Concatenate the 2 nodes together
            if !build_extensions {
                match &self.value {
                    HashBuilderValue::Bytes(leaf_value) => {
                        let leaf_node = LeafNode::new(&short_node_key, leaf_value);
                        tracing::debug!(?leaf_node, "pushing leaf node");
                        tracing::trace!(rlp = hex::encode(&leaf_node.rlp()), "leaf node rlp");
                        self.stack.push(leaf_node.rlp());
                    }
                    HashBuilderValue::Hash(hash) => {
                        tracing::debug!(?hash, "pushing branch node hash");
                        self.stack.push(rlp_hash(*hash));

                        if self.is_in_db_trie {
                            self.tree_masks[current.len() - 1] |= 1u16 << current.last().unwrap();
                        }
                        self.hash_masks[current.len() - 1] |= 1u16 << current.last().unwrap();

                        build_extensions = true;
                    }
                }
            }

            if build_extensions && !short_node_key.is_empty() {
                self.update_masks(&current, len_from);
                let stack_last =
                    self.stack.pop().expect("there should be at least one stack item; qed");
                let extension_node = ExtensionNode::new(&short_node_key, &stack_last);
                tracing::debug!(?extension_node, "pushing extension node");
                tracing::trace!(rlp = hex::encode(&extension_node.rlp()), "extension node rlp");
                self.stack.push(extension_node.rlp());
                self.resize_masks(len_from);
            }

            if preceding_len <= common_prefix_len && !succeeding.is_empty() {
                tracing::trace!("no common prefix to create branch nodes from, returning");
                return
            }

            // Insert branch nodes in the stack
            if !succeeding.is_empty() || preceding_exists {
                // Need to store the branch node in an efficient format
                // outside of the hash builder
                self.store_branch_node(&current, len);
                // Pushes the corresponding branch node to the stack
                self.push_branch_node(len);
            }

            self.groups.resize(len, 0u16);
            self.resize_masks(len);

            if preceding_len == 0 {
                tracing::trace!("0 or 1 state masks means we have no more elements to process");
                return
            }

            current.truncate(preceding_len);
            tracing::trace!(?current, "truncated nibbles to {} bytes", preceding_len);

            tracing::trace!(?self.groups, "popping empty state masks");
            while self.groups.last() == Some(&0) {
                self.groups.pop();
            }

            build_extensions = true;

            i += 1;
        }
    }

    /// Given the size of the longest common prefix, it proceeds to create a branch node
    /// from the state mask and existing stack state, and store its RLP to the top of the stack,
    /// after popping all the relevant elements from the stack.
    fn push_branch_node(&mut self, len: usize) {
        let state_mask = self.groups[len];
        let rlp = BranchNode::new(&self.stack).rlp(state_mask);

        // Clears the stack from the branch node elements
        let first_child_idx = self.stack.len() - state_mask.count_ones() as usize;
        tracing::debug!(
            new_len = first_child_idx,
            old_len = self.stack.len(),
            "resizing stack to prepare branch node"
        );
        self.stack.resize(first_child_idx, vec![]);

        tracing::debug!("pushing branch node with {:b} mask from stack", state_mask);
        tracing::trace!(rlp = hex::encode(&rlp), "branch node rlp");
        self.stack.push(rlp);
    }

    /// Given the current nibble prefix and the highest common prefix length, proceeds
    /// to update the masks for the next level and store the branch node and the
    /// masks in the database. We will use that when consuming the intermediate nodes
    /// from the database to efficiently build the trie.
    fn store_branch_node(&mut self, current: &Nibbles, len: usize) {
        let state_mask = self.groups[len];
        let hash_mask = self.hash_masks[len];
        let hashes = BranchNode::new(&self.stack)
            .children(state_mask, hash_mask)
            .map(|hash| H256::from_slice(&hash[1..]))
            .collect();

        if len > 0 {
            self.hash_masks[len - 1] |= 1u16 << current[len - 1];
        }

        let store_in_db_trie = self.tree_masks[len] != 0 || self.hash_masks[len] != 0;
        if store_in_db_trie {
            if len > 0 {
                self.tree_masks[len - 1] |= 1u16 << current[len - 1];
            }

            let mut n = BranchNodeCompact::new(
                self.groups[len],
                self.tree_masks[len],
                self.hash_masks[len],
                hashes,
                None,
            );

            if len == 0 {
                n.root_hash = Some(self.current_root());
            }

            // Send it over to the provided channel which will handle it on the
            // other side of the HashBuilder
            tracing::debug!(node = ?n, "intermediate node");
        }
    }

    fn update_masks(&mut self, current: &Nibbles, len_from: usize) {
        if len_from > 0 {
            let flag = 1u16 << current[len_from - 1];

            self.hash_masks[len_from - 1] &= !flag;

            if self.tree_masks[current.len() - 1] != 0 {
                self.tree_masks[len_from - 1] |= flag;
            }
        }
    }

    fn resize_masks(&mut self, new_len: usize) {
        tracing::trace!(
            new_len,
            old_tree_mask_len = self.tree_masks.len(),
            old_hash_mask_len = self.hash_masks.len(),
            "resizing tree/hash masks"
        );
        self.tree_masks.resize(new_len, 0u16);
        self.hash_masks.resize(new_len, 0u16);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trie_v2::nibbles::Nibbles;
    use hex_literal::hex;
    use reth_primitives::{proofs::KeccakHasher, H256, U256};
    use std::{
        collections::{BTreeMap, HashMap},
        str::FromStr,
    };

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

        let mut hb = HashBuilder::new();

        hashed.iter().for_each(|(key, val)| {
            let nibbles = Nibbles::unpack(key);
            hb.add_leaf(nibbles, &val);
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
        let mut hb = HashBuilder::new();

        let data = iter.collect::<BTreeMap<_, _>>();
        data.iter().for_each(|(key, val)| {
            let nibbles = Nibbles::unpack(key);
            hb.add_leaf(nibbles, val.as_ref());
        });
        assert_eq!(hb.root(), trie_root(data));
    }

    #[test]
    fn empty() {
        assert_eq!(HashBuilder::new().root(), EMPTY_ROOT);
    }

    // TODO: Expand these to include more complex cases.
    #[test]
    fn test_root_raw_data() {
        reth_tracing::init_test_tracing();
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
        let root_hash =
            H256::from_str("9fa752911d55c3a1246133fe280785afbdba41f357e9cae1131d5f5b0a078b9c")
                .unwrap();

        let mut hb = HashBuilder::new();
        hb.add_branch(Nibbles::default(), root_hash);
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
        let mut hb = HashBuilder::new();
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

        let mut hb2 = HashBuilder::new();
        // Insert the branch with the `0x6` shared prefix.
        hb2.add_branch(Nibbles::from_hex(vec![0x6]), branch_node_hash);

        let expected = trie_root(raw_input.clone());
        assert_eq!(hb.root(), expected);
        assert_eq!(hb2.root(), expected);
    }
}
