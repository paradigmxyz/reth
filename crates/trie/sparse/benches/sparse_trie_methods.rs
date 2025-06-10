#![allow(missing_docs)]

use alloy_primitives::{map::B256Map, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie_common::{Nibbles, TrieNode};
use reth_trie_sparse::{SparseTrie, TrieMasks};

const LEAF_COUNT: usize = 10_000;

/// Benchmarks for individual sparse trie methods.
fn sparse_trie_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("sparse trie methods");
    group.sample_size(20);

    // Generate test data with 10k leaves
    let test_data = generate_test_data(LEAF_COUNT);
    let encoded_data: Vec<(Nibbles, Vec<u8>)> = test_data
        .iter()
        .map(|(key, value)| (Nibbles::unpack(key), alloy_rlp::encode_fixed_size(value).to_vec()))
        .collect();

    // Benchmark: reveal_root (converting blind trie to revealed)
    // Since we can't use reveal_node directly on SparseTrie enum, we'll benchmark reveal_root
    // which converts a blind trie to a revealed one
    group.bench_function(BenchmarkId::new("reveal_root", LEAF_COUNT), |b| {
        b.iter_with_setup(
            || {
                // Create a new blind trie for each iteration
                SparseTrie::blind()
            },
            |mut trie| {
                // Reveal the root with an empty branch node
                let _ = trie.reveal_root(
                    TrieNode::Branch(Default::default()),
                    TrieMasks::none(),
                    false,
                );
                trie
            },
        );
    });

    // Benchmark: reveal_node on RevealedSparseTrie
    // This benchmarks revealing nodes within an already revealed trie
    group.bench_function(BenchmarkId::new("reveal_node", LEAF_COUNT), |b| {
        b.iter_with_setup(
            || {
                // Create a revealed trie with a branch root
                let mut trie = SparseTrie::revealed_empty();
                if let SparseTrie::Revealed(revealed) = &mut trie {
                    // Add some initial structure to make reveal_node meaningful
                    let _ = revealed.reveal_node(
                        Nibbles::default(),
                        TrieNode::Branch(Default::default()),
                        TrieMasks::none(),
                    );
                }
                trie
            },
            |mut trie| {
                if let SparseTrie::Revealed(revealed) = &mut trie {
                    // Reveal a node at a specific path
                    let path = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
                    // Create a leaf node with empty value
                    let leaf = TrieNode::Leaf(reth_trie_common::LeafNode {
                        key: path.clone(),
                        value: vec![],
                    });
                    let _ = revealed.reveal_node(
                        path,
                        leaf,
                        TrieMasks::none(),
                    );
                }
                trie
            },
        );
    });

    // Benchmark: update_leaf
    group.bench_function(BenchmarkId::new("update_leaf", LEAF_COUNT), |b| {
        b.iter_with_setup(
            || {
                // Start with an empty trie
                let mut trie = SparseTrie::revealed_empty();
                // Pre-populate with data
                for (path, value) in &encoded_data {
                    trie.update_leaf(path.clone(), value.clone()).unwrap();
                }
                trie
            },
            |mut trie| {
                // Update existing leaves with new values
                for (i, (path, _)) in encoded_data.iter().enumerate().take(100) {
                    let new_value = alloy_rlp::encode_fixed_size(&U256::from(i * 2)).to_vec();
                    trie.update_leaf(path.clone(), new_value).unwrap();
                }
                trie
            },
        );
    });

    // Benchmark: remove_leaf
    group.bench_function(BenchmarkId::new("remove_leaf", LEAF_COUNT), |b| {
        b.iter_with_setup(
            || {
                // Start with a populated trie
                let mut trie = SparseTrie::revealed_empty();
                for (path, value) in &encoded_data {
                    trie.update_leaf(path.clone(), value.clone()).unwrap();
                }
                trie
            },
            |mut trie| {
                // Remove 100 leaves
                for (path, _) in encoded_data.iter().take(100) {
                    trie.remove_leaf(path).unwrap();
                }
                trie
            },
        );
    });

    // Benchmark: root calculation
    group.bench_function(BenchmarkId::new("root", LEAF_COUNT), |b| {
        b.iter_with_setup(
            || {
                // Pre-populate trie
                let mut trie = SparseTrie::revealed_empty();
                for (path, value) in &encoded_data {
                    trie.update_leaf(path.clone(), value.clone()).unwrap();
                }
                trie
            },
            |mut trie| {
                // Calculate root
                let _root = trie.root().unwrap();
                trie
            },
        );
    });

    // Additional benchmarks for different trie sizes to show scaling
    for size in [1_000, 5_000, 10_000] {
        let subset_data: Vec<_> = encoded_data.iter().take(size).cloned().collect();
        
        group.bench_function(BenchmarkId::new("build_and_root", size), |b| {
            b.iter_with_setup(
                || SparseTrie::revealed_empty(),
                |mut trie| {
                    // Build trie and calculate root
                    for (path, value) in &subset_data {
                        trie.update_leaf(path.clone(), value.clone()).unwrap();
                    }
                    let _root = trie.root().unwrap();
                    trie
                },
            );
        });
    }
}

/// Generate deterministic test data
fn generate_test_data(size: usize) -> B256Map<U256> {
    let mut runner = TestRunner::deterministic();
    proptest::collection::hash_map(any::<B256>(), any::<U256>(), size)
        .new_tree(&mut runner)
        .unwrap()
        .current()
        .into_iter()
        .collect()
}

criterion_group!(benches, sparse_trie_methods);
criterion_main!(benches);