//! Streaming construction of complete tries from sorted leaves.
use crate::{Nibbles, RlpNode, StateTrieNode, TrieMask, EMPTY_ROOT_HASH};
use alloc::vec::Vec;
use alloy_primitives::{keccak256, B256};
use alloy_rlp::Encodable;

/// Builds complete tries in one pass with at most 64 open branches.
#[derive(Debug)]
pub struct StateTrieBuilder<V> {
    branches: Vec<(Nibbles, TrieMask, Vec<RlpNode>)>,
    pending: Option<(Nibbles, StateTrieNode<V>)>,
    last: Option<Nibbles>,
}

impl<V> Default for StateTrieBuilder<V> {
    fn default() -> Self {
        Self { branches: Vec::new(), pending: None, last: None }
    }
}

impl<V: Encodable + Clone> StateTrieBuilder<V> {
    /// Add a leaf in strictly increasing hashed-key order. Emits completed nodes to `write`.
    pub fn push<E>(
        &mut self,
        key: B256,
        value: V,
        write: &mut impl FnMut(Nibbles, StateTrieNode<V>) -> Result<(), E>,
    ) -> Result<(), E> {
        let key = Nibbles::unpack(key);
        if let Some(last) = self.last {
            assert!(key > last, "state trie leaves must be strictly sorted");
            let common = last.common_prefix_length(&key);
            while self.branches.last().is_some_and(|(p, _, _)| p.len() > common) {
                self.close_branch(write)?;
            }
            if self.branches.last().is_none_or(|(p, _, _)| p.len() < common) {
                self.branches.push((
                    key.slice(..common),
                    TrieMask::default(),
                    Vec::with_capacity(16),
                ));
            }
            self.commit_child(write)?;
        }
        self.pending = Some((key, StateTrieNode::Leaf { short_key_len: 64, value }));
        self.last = Some(key);
        Ok(())
    }

    /// Emit all remaining nodes and return the root hash.
    pub fn finish<E>(
        mut self,
        write: &mut impl FnMut(Nibbles, StateTrieNode<V>) -> Result<(), E>,
    ) -> Result<B256, E> {
        while !self.branches.is_empty() {
            self.close_branch(write)?;
        }
        let Some((path, mut node)) = self.pending.take() else { return Ok(EMPTY_ROOT_HASH) };
        match &mut node {
            StateTrieNode::Leaf { short_key_len, .. } |
            StateTrieNode::Branch { short_key_len, .. } => *short_key_len = path.len() as u8,
        }
        let root = keccak256(alloy_rlp::encode(node.clone().proof_node(path).node));
        write(path, node)?;
        Ok(root)
    }

    fn commit_child<E>(
        &mut self,
        write: &mut impl FnMut(Nibbles, StateTrieNode<V>) -> Result<(), E>,
    ) -> Result<(), E> {
        let (path, mut node) = self.pending.take().expect("pending child");
        let (parent, mask, children) = self.branches.last_mut().expect("parent branch");
        let short_len = (path.len() - parent.len() - 1) as u8;
        match &mut node {
            StateTrieNode::Leaf { short_key_len, .. } |
            StateTrieNode::Branch { short_key_len, .. } => *short_key_len = short_len,
        }
        mask.set_bit(path.get_unchecked(parent.len()));
        let encoded = alloy_rlp::encode(node.clone().proof_node(path).node);
        children.push(RlpNode::from_rlp(&encoded));
        write(path, node)
    }

    fn close_branch<E>(
        &mut self,
        write: &mut impl FnMut(Nibbles, StateTrieNode<V>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.commit_child(write)?;
        let (path, state_mask, children) = self.branches.pop().expect("open branch");
        self.pending =
            Some((path, StateTrieNode::Branch { short_key_len: 0, state_mask, children }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloy_primitives::U256;
    #[test]
    fn complete_nodes_match_reference_root() {
        for count in [0, 1, 2, 16, 256, 2048] {
            let leaves: BTreeMap<_, _> =
                (0u64..count).map(|i| (keccak256(i.to_be_bytes()), U256::from(i + 1))).collect();
            let expected = crate::root::storage_root(leaves.iter().map(|(k, v)| (*k, *v)));
            let mut nodes = BTreeMap::new();
            let mut write = |path, node| {
                assert!(nodes.insert(path, node).is_none());
                Ok::<_, core::convert::Infallible>(())
            };
            let mut builder = StateTrieBuilder::default();
            for (k, v) in &leaves {
                builder.push(*k, *v, &mut write).unwrap();
            }
            assert_eq!(builder.finish(&mut write).unwrap(), expected);
            for (path, node) in &nodes {
                let proof = node.clone().proof_node(*path);
                if !proof.path.is_empty() {
                    let parent = proof.path.slice(..proof.path.len() - 1);
                    assert!(matches!(nodes.get(&parent), Some(StateTrieNode::Branch { .. })));
                }
            }
        }
    }
}
