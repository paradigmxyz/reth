#![allow(missing_docs)]

use alloy_primitives::{B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree};
use reth_trie_common::Nibbles;
use reth_trie_sparse::SparseTrie;

const LEAF_COUNTS: [usize; 3] = [100, 1_000, 5_000];

fn update_leaf(c: &mut Criterion) {
    let mut group = c.benchmark_group("update_leaf");

    for leaf_count in LEAF_COUNTS {
        group.bench_function(BenchmarkId::from_parameter(leaf_count), |b| {
            let leaves = generate_leaves(leaf_count);
            b.iter_with_setup(
                || {
                    // Start with an empty trie
                    let mut trie = SparseTrie::revealed_empty();
                    // Pre-populate with data
                    for (path, value) in &leaves {
                        trie.update_leaf(path.clone(), value.clone()).unwrap();
                    }
                    trie
                },
                |mut trie| {
                    // Update existing leaves with new values
                    for (i, (path, _)) in leaves.iter().enumerate().take(100) {
                        let new_value = alloy_rlp::encode_fixed_size(&U256::from(i * 2)).to_vec();
                        trie.update_leaf(path.clone(), new_value).unwrap();
                    }
                    trie
                },
            );
        });
    }
}

fn remove_leaf(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_leaf");

    for leaf_count in LEAF_COUNTS {
        group.bench_function(BenchmarkId::from_parameter(leaf_count), |b| {
            let leaves = generate_leaves(leaf_count);
            b.iter_with_setup(
                || {
                    // Start with an empty trie
                    let mut trie = SparseTrie::revealed_empty();
                    // Pre-populate with data
                    for (path, value) in &leaves {
                        trie.update_leaf(path.clone(), value.clone()).unwrap();
                    }
                    trie
                },
                |mut trie| {
                    // Remove 100 leaves
                    for (path, _) in leaves.iter().take(100) {
                        trie.remove_leaf(path).unwrap();
                    }
                    trie
                },
            );
        });
    }
}

fn generate_leaves(size: usize) -> Vec<(Nibbles, Vec<u8>)> {
    proptest::collection::hash_map(any::<B256>(), any::<U256>(), size)
        .new_tree(&mut Default::default())
        .unwrap()
        .current()
        .iter()
        .map(|(key, value)| (Nibbles::unpack(key), alloy_rlp::encode_fixed_size(value).to_vec()))
        .collect()
}

criterion_group!(benches, update_leaf, remove_leaf);
criterion_main!(benches);
