#![allow(unused)]
use std::default;

use reth_primitives::{keccak256, proofs::EMPTY_ROOT, H256};

use crate::trie_v2::node::{rlp_hash, BranchNode, ExtensionNode, LeafNode, KECCAK_LENGTH};

use super::nibbles::Nibbles;

#[derive(Clone, Debug)]
enum HashBuilderValue {
    Bytes(Vec<u8>),
    Hash(H256),
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

    // TODO: Add the remaining masks for on disk persistence
    groups: Vec<u16>,
}

impl HashBuilder {
    pub fn stack_hex(&self) {
        println!("STACK");
        for item in &self.stack {
            println!("{}", hex::encode(item));
        }
    }
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_leaf(&mut self, key: Nibbles, value: &[u8]) {
        assert!(key > self.key);
        self.add(key, value);
    }

    pub fn add_branch(&mut self, key: Nibbles, value: H256) {
        assert!(key > self.key || (self.key.is_empty() && key.is_empty()));
        if self.key.is_empty() {
            self.stack.push(rlp_hash(value));
        }
        self.add(key, value);
    }

    fn add<T: Into<HashBuilderValue>>(&mut self, key: Nibbles, value: T) {
        if !self.key.is_empty() {
            self.gen_struct_step(&key);
        }
        self.key = key;
        self.value = value.into();
    }

    /// Returns the current root hash of the trie builder.
    fn root(&mut self) -> H256 {
        // Clears the internal state
        if !self.key.is_empty() {
            self.gen_struct_step(&Nibbles::default());
            self.key.clear();
            self.value = HashBuilderValue::Bytes(vec![]);
        }

        self.stack_hex();

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

    // 1. Updates the hash calculation stack with the
    fn gen_struct_step(&mut self, succeeding: &Nibbles) {
        let mut build_extensions = false;
        // current / self.key is always the latest added element in the trie
        let mut current = self.key.clone();

        let mut i = 0;
        loop {
            println!("Looping {i}");
            i += 1;
            let preceding_exists = !self.groups.is_empty();
            let preceding_len: usize = self.groups.len().saturating_sub(1);

            let common_prefix_len = succeeding.prefix_length(&current);
            let len = std::cmp::max(preceding_len, common_prefix_len);
            assert!(len < current.len());

            let extra_digit = current[len];
            if self.groups.len() <= len {
                self.groups.resize(len + 1, 0u16);
            }
            self.groups[len] |= 1u16 << extra_digit;

            let mut len_from = len;
            if !succeeding.is_empty() || preceding_exists {
                len_from += 1;
            }

            // The key without the common prefix
            let short_node_key = current.offset(len_from);

            // Concatenate the 2 nodes together
            if !build_extensions {
                let value = self.value.clone();
                match &value {
                    HashBuilderValue::Bytes(leaf_value) => {
                        dbg!(&short_node_key);
                        let leaf_node = LeafNode::new(&short_node_key, leaf_value);
                        dbg!(&leaf_node);
                        println!("[+] Pushing leaf node: {:?}", hex::encode(&leaf_node.rlp()));
                        self.stack.push(leaf_node.rlp());
                    }
                    HashBuilderValue::Hash(hash) => {
                        self.stack.push(rlp_hash(*hash));
                        build_extensions = true;
                    }
                }
            }

            if build_extensions && !short_node_key.is_empty() {
                let stack_last = self.stack.pop().unwrap();
                println!("[-] Popping stack top: {:?}", hex::encode(&stack_last));
                println!("[-] Short Node KEy {:?}", short_node_key);
                let extension_node = ExtensionNode::new(&short_node_key, &stack_last);

                println!("[+] Pushing extension node: {:?}", hex::encode(&extension_node.rlp()));
                self.stack.push(extension_node.rlp());
            }

            if preceding_len <= common_prefix_len && !succeeding.is_empty() {
                return
            }

            // Insert branch nodes in the stack
            if !succeeding.is_empty() || preceding_exists {
                let state_mask = self.groups[len];
                self.stack_hex();
                let rlp = BranchNode::new(&self.stack).rlp(self.groups[len]);

                // Clears the stack from the branch node elements
                let first_child_idx = self.stack.len() - state_mask.count_ones() as usize;
                self.stack.resize(first_child_idx, vec![]);
                self.stack.push(rlp);
            }

            self.groups.resize(len, 0u16);

            if preceding_len == 0 {
                return
            }

            current.truncate(preceding_len);
            while self.groups.last() == Some(&0) {
                self.groups.pop();
            }

            build_extensions = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::trie_v2::nibbles::Nibbles;
    use hex_literal::hex;
    use std::str::FromStr;

    use super::*;
    use reth_primitives::proofs::KeccakHasher;

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

    #[test]
    fn empty() {
        assert_eq!(HashBuilder::new().root(), EMPTY_ROOT);
    }

    #[test]
    fn test_hash_builder_1() {
        let data = vec![
            (hex!("646f").to_vec(), hex!("76657262").to_vec()),
            (hex!("676f6f64").to_vec(), hex!("7075707079").to_vec()),
        ];

        let mut hb = HashBuilder::new();
        for (key, val) in data.iter() {
            let nibbles = Nibbles::unpack(key);
            hb.add_leaf(nibbles, val.as_slice());
            hb.stack_hex();
        }

        let root_hash = hb.root();
        assert_eq!(root_hash, trie_root(data));
    }

    #[test]
    fn test_hash_builder_known_root_hash() {
        let root_hash =
            H256::from_str("9fa752911d55c3a1246133fe280785afbdba41f357e9cae1131d5f5b0a078b9c")
                .unwrap();

        let mut hb = HashBuilder::new();
        hb.add_branch(Nibbles::default(), root_hash);
        assert_eq!(hb.root(), root_hash);
    }
}
