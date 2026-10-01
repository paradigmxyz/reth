use super::*;
use reth_trie_common::{StateTrieBuilder, StateTrieNode};

fn complete_nodes(
    storage: &BTreeMap<B256, U256>,
) -> BTreeMap<Nibbles, StateTrieNode<smallvec::SmallVec<[u8; 16]>>> {
    let mut builder = StateTrieBuilder::default();
    let mut nodes = BTreeMap::new();
    let mut write = |path, node| {
        let node = match node {
            StateTrieNode::Leaf { short_key_len, value } => {
                StateTrieNode::Leaf { short_key_len, value: alloy_rlp::encode(value).into() }
            }
            StateTrieNode::Branch { short_key_len, state_mask, children } => {
                StateTrieNode::Branch { short_key_len, state_mask, children }
            }
        };
        nodes.insert(path, node);
        Ok::<_, core::convert::Infallible>(())
    };
    for (k, v) in storage {
        builder.push(*k, *v, &mut write).unwrap();
    }
    builder.finish(&mut write).unwrap();
    nodes
}

pub(super) fn test_complete_updates_match_rebuild<T: SparseTrie>(new_trie: fn() -> T) {
    for shared_prefix in [0, 1, 2, 31] {
        let mut storage = BTreeMap::new();
        for i in 1u64..1025 {
            let mut key = alloy_primitives::keccak256(i.to_be_bytes());
            key.0[..shared_prefix].fill(0);
            storage.insert(key, U256::from(i));
        }
        let mut nodes = complete_nodes(&storage);
        let mut harness = SuiteTestHarness::new(storage.clone());
        let root = harness.root_node();
        let mut trie = new_trie();
        trie.set_state_trie_updates(true);
        trie.set_root(root.node, root.masks, false).unwrap();
        assert!(trie.take_state_trie_updates().is_empty());
        for round in 1..=5 {
            let changes: BTreeMap<_, _> = if round == 5 {
                storage.keys().map(|k| (*k, U256::ZERO)).collect()
            } else {
                let mut changes: BTreeMap<_, _> = storage
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % 3 == 0)
                    .map(|(i, (k, _))| {
                        (*k, if i % 2 == 0 { U256::ZERO } else { U256::from(round * 100) })
                    })
                    .collect();
                for i in 1100u64..1150 {
                    let key = alloy_primitives::keccak256((i * round).to_be_bytes());
                    changes.insert(key, U256::from(i));
                }
                changes
            };
            let mut leaf_updates = SuiteTestHarness::leaf_updates(&changes);
            harness.reveal_and_update(&mut trie, &mut leaf_updates);
            for (k, v) in changes {
                if v.is_zero() {
                    storage.remove(&k);
                } else {
                    storage.insert(k, v);
                }
            }
            harness = SuiteTestHarness::new(storage.clone());
            assert_eq!(
                trie.root(epoch(round)),
                reth_trie_common::root::storage_root(storage.iter().map(|(k, v)| (*k, *v)))
            );
            for (p, n) in trie.take_state_trie_updates() {
                if let Some(n) = n {
                    nodes.insert(p, n);
                } else {
                    nodes.remove(&p);
                }
            }
            assert_eq!(
                nodes,
                complete_nodes(&storage),
                "round={round} shared_prefix={shared_prefix}"
            );
            assert!(trie.take_state_trie_updates().is_empty());
            assert!(trie.take_updates().updated_nodes.is_empty());
            trie.prune(epoch(round));
        }
        trie.clear();
        assert!(trie.take_state_trie_updates().is_empty());
    }
}
