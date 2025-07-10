#![allow(missing_docs)]

use alloy_primitives::{B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree};
use rand::seq::IteratorRandom;
use reth_trie_common::Nibbles;
use reth_trie_sparse::{blinded::DefaultBlindedProvider, SerialSparseTrie, SparseTrie};

const LEAF_COUNTS: [usize; 2] = [1_000, 5_000];

fn update_leaf(c: &mut Criterion) {
    let mut group = c.benchmark_group("update_leaf");

    for leaf_count in LEAF_COUNTS {
        group.bench_function(BenchmarkId::from_parameter(leaf_count), |b| {
            let leaves = generate_leaves(leaf_count);
            // Start with an empty trie
            let provider = DefaultBlindedProvider;

            b.iter_batched(
                || {
                    let mut trie = SparseTrie::<SerialSparseTrie>::revealed_empty();
                    // Pre-populate with data
                    for (path, value) in leaves.iter().cloned() {
                        trie.update_leaf(path, value, &provider).unwrap();
                    }

                    let new_leaves = leaves
                        .iter()
                        // Update 10% of existing leaves with new values
                        .choose_multiple(&mut rand::rng(), leaf_count / 10)
                        .into_iter()
                        .map(|(path, _)| {
                            (
                                path,
                                alloy_rlp::encode_fixed_size(&U256::from(path.len() * 2)).to_vec(),
                            )
                        })
                        .collect::<Vec<_>>();

                    (trie, new_leaves)
                },
                |(mut trie, new_leaves)| {
                    for (path, new_value) in new_leaves {
                        trie.update_leaf(*path, new_value, &provider).unwrap();
                    }
                    trie
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn remove_leaf(c: &mut Criterion) {
    let mut group = c.benchmark_group("remove_leaf");

    for leaf_count in LEAF_COUNTS {
        group.bench_function(BenchmarkId::from_parameter(leaf_count), |b| {
            let leaves = generate_leaves(leaf_count);
            // Start with an empty trie
            let provider = DefaultBlindedProvider;

            b.iter_batched(
                || {
                    let mut trie = SparseTrie::<SerialSparseTrie>::revealed_empty();
                    // Pre-populate with data
                    for (path, value) in leaves.iter().cloned() {
                        trie.update_leaf(path, value, &provider).unwrap();
                    }

                    let delete_leaves = leaves
                        .iter()
                        .map(|(path, _)| path)
                        // Remove 10% leaves
                        .choose_multiple(&mut rand::rng(), leaf_count / 10);

                    (trie, delete_leaves)
                },
                |(mut trie, delete_leaves)| {
                    for path in delete_leaves {
                        trie.remove_leaf(path, &provider).unwrap();
                    }
                    trie
                },
                BatchSize::LargeInput,
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
