use alloy_primitives::{B256, U256};
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    hash_builder::{HashBuilderValue, HashBuilderValueRef},
    hashed_cursor::HashedCursor,
    node_iter::{TrieElement, TrieNodeIter},
    prefix_set::PrefixSet,
    trie_cursor::TrieCursor,
    walker::TrieWalker,
    Nibbles, RlpNode, TrieMask,
};
use reth_trie_sparse::{RevealedSparseTrie, SparseNode, SparseTrie};

// TODO:

#[derive(Debug, Default)]
pub struct SparseTrieBuilder {
    key: Nibbles,
    value: HashBuilderValue,
    state_masks: Vec<TrieMask>,
    trie: RevealedSparseTrie,
}

impl SparseTrieBuilder {
    fn add_branch(&mut self, key: Nibbles, value: B256) {
        assert!(key > self.key || (self.key.is_empty() && key.is_empty()));
        if !self.key.is_empty() {
            self.update(&key);
        } else if key.is_empty() {
            self.trie.insert_node_unchecked(key.clone(), SparseNode::Hash(value));
        }
        self.set_key_value(key, HashBuilderValueRef::Hash(&value));
    }

    fn add_leaf(&mut self, key: Nibbles, value: &[u8]) {
        assert!(key > self.key);
        if !self.key.is_empty() {
            self.update(&key);
        }
        self.set_key_value(key, HashBuilderValueRef::Bytes(value));
    }

    fn set_key_value(&mut self, key: Nibbles, value: HashBuilderValueRef<'_>) {
        self.key = key;
        self.value.set_from_ref(value);
    }

    fn update(&mut self, succeeding: &Nibbles) {
        let mut build_extensions = false;
        let mut current = self.key.clone();

        loop {
            let preceding_exists = !self.state_masks.is_empty();
            let preceding_len = self.state_masks.len().saturating_sub(1);

            let common_prefix_len = succeeding.common_prefix_length(&current);
            let len = std::cmp::max(preceding_len, common_prefix_len);

            let extra_digit = current[len];
            if self.state_masks.len() <= len {
                self.state_masks.resize(len + 1, TrieMask::default());
            }
            self.state_masks[len].set_bit(extra_digit);

            let mut len_from = len;
            if !succeeding.is_empty() || preceding_exists {
                len_from += 1;
            }

            let short_node_key = current.slice(len_from..);

            if !build_extensions {
                match self.value.as_ref() {
                    HashBuilderValueRef::Bytes(leaf_value) => {
                        let leaf = SparseNode::new_leaf(short_node_key.clone());
                        self.trie.insert_node_unchecked(
                            current.slice(..len_from),
                            SparseNode::new_leaf(short_node_key.clone()),
                        );
                        self.trie.insert_value_unchecked(current.clone(), leaf_value.to_vec());
                    }
                    HashBuilderValueRef::Hash(hash) => {
                        // TODO: check
                        println!("trying to insert hash at {current:?}");
                        self.trie.insert_node_unchecked(current.clone(), SparseNode::Hash(*hash));
                        build_extensions = true;
                    }
                }
            }

            if build_extensions && !short_node_key.is_empty() {
                let ext = SparseNode::new_ext(short_node_key);
                println!("inserting extension at {:?}", current.slice(..len_from));
                self.trie.insert_node_unchecked(current.slice(..len_from), ext);
            }

            if preceding_len <= common_prefix_len && !succeeding.is_empty() {
                return;
            }

            if !succeeding.is_empty() || preceding_exists {
                let branch = SparseNode::new_branch(self.state_masks[len]);
                self.trie.insert_node_unchecked(current.slice(..len), branch);
            }

            self.state_masks.resize(len, TrieMask::default());

            if preceding_len == 0 {
                return;
            }

            current.truncate(preceding_len);
            while self.state_masks.last() == Some(&TrieMask::default()) {
                self.state_masks.pop();
            }

            build_extensions = true;
        }
    }

    pub fn build<T, H>(
        trie_cursor: T,
        hashed_cursor: H,
        prefix_set: PrefixSet,
    ) -> Result<RevealedSparseTrie, DatabaseError>
    where
        T: TrieCursor,
        H: HashedCursor<Value = U256>,
    {
        let mut builder = Self::default();
        let walker = TrieWalker::new(trie_cursor, prefix_set);
        let mut node_iter = TrieNodeIter::new(walker, hashed_cursor);
        while let Some(node) = node_iter.try_next()? {
            match node {
                TrieElement::Branch(node) => {
                    assert!(
                        node.key > builder.key || (builder.key.is_empty() && node.key.is_empty())
                    );
                    if !builder.key.is_empty() {
                        builder.update(&node.key);
                    } else if node.key.is_empty() {
                        builder
                            .trie
                            .insert_node_unchecked(node.key.clone(), SparseNode::Hash(node.value));
                    }
                    builder.key = node.key;
                    builder.value.set_from_ref(HashBuilderValueRef::Hash(&node.value));
                }
                TrieElement::Leaf(hashed_slot, value) => {
                    let key = Nibbles::unpack(hashed_slot);
                    builder.add_leaf(key, alloy_rlp::encode_fixed_size(&value).as_ref());
                }
            }
        }
        builder.update(&Nibbles::default());
        Ok(builder.trie)
    }
}
