//! Benchmark comparing Schwartzian transform vs direct sorting for ProofTrieNode sorting.
//!
//! The Schwartzian transform (decorate-sort-undecorate) precomputes the sort key once per element
//! before sorting, avoiding repeated key computations during the sort's comparisons.
//!
//! This benchmark tests whether the optimization is beneficial for `SparseSubtrieType::from_path`,
//! which is a very cheap operation (just a length check + byte read).

#![allow(missing_docs)]

use alloy_trie::Nibbles;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::Rng;
use reth_trie_common::{LeafNode, ProofTrieNode, TrieNode};

const UPPER_TRIE_MAX_DEPTH: usize = 2;

/// Sparse Subtrie Type - copied from trie.rs for benchmark isolation
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
enum SparseSubtrieType {
    Upper,
    Lower(usize),
}

impl SparseSubtrieType {
    const fn path_len_is_upper(len: usize) -> bool {
        len < UPPER_TRIE_MAX_DEPTH
    }

    fn from_path(path: &Nibbles) -> Self {
        if Self::path_len_is_upper(path.len()) {
            Self::Upper
        } else {
            // path.get_byte_unchecked(0) as usize
            Self::Lower(path.get_byte_unchecked(0) as usize)
        }
    }
}

/// Generate random test data mimicking realistic ProofTrieNode distributions
fn generate_test_nodes(count: usize) -> Vec<ProofTrieNode> {
    let mut rng = rand::rng();
    let mut nodes = Vec::with_capacity(count);

    for _ in 0..count {
        // Generate paths with varying lengths (0-64 nibbles, typical for Ethereum tries)
        let path_len = rng.random_range(0..=64);
        let path_bytes: Vec<u8> = (0..path_len).map(|_| rng.random_range(0..16)).collect();
        let path = Nibbles::from_nibbles_unchecked(path_bytes);

        // Create a simple leaf node for the benchmark
        let value = vec![rng.random::<u8>(); 32];
        let key = Nibbles::from_nibbles_unchecked(vec![0u8; 4]);
        let node = TrieNode::Leaf(LeafNode::new(key, value));

        nodes.push(ProofTrieNode { path, node, masks: None });
    }

    nodes
}

/// Original approach: direct sorting with repeated from_path calls
fn sort_direct(nodes: &mut Vec<ProofTrieNode>) {
    nodes.sort_unstable_by(
        |ProofTrieNode { path: path_a, .. }, ProofTrieNode { path: path_b, .. }| {
            let subtrie_type_a = SparseSubtrieType::from_path(path_a);
            let subtrie_type_b = SparseSubtrieType::from_path(path_b);
            subtrie_type_a.cmp(&subtrie_type_b).then(path_a.cmp(path_b))
        },
    );
}

/// Schwartzian transform: decorate-sort-undecorate
fn sort_schwartzian(nodes: &mut Vec<ProofTrieNode>) {
    let mut decorated: Vec<_> =
        nodes.drain(..).map(|n| (SparseSubtrieType::from_path(&n.path), n)).collect();
    decorated.sort_unstable_by(|(type_a, node_a), (type_b, node_b)| {
        type_a.cmp(type_b).then(node_a.path.cmp(&node_b.path))
    });
    nodes.extend(decorated.into_iter().map(|(_, node)| node));
}

/// Alternative: sort_by_cached_key (standard library's built-in Schwartzian)
fn sort_cached_key(nodes: &mut Vec<ProofTrieNode>) {
    nodes.sort_by_cached_key(|n| (SparseSubtrieType::from_path(&n.path), n.path.clone()));
}

fn bench_sort_approaches(c: &mut Criterion) {
    let mut group = c.benchmark_group("ProofTrieNode sorting");
    group.sample_size(50);

    for size in [100, 1_000, 10_000] {
        let test_nodes = generate_test_nodes(size);

        group.bench_function(BenchmarkId::new("direct", size), |b| {
            b.iter_batched(
                || test_nodes.clone(),
                |mut nodes| {
                    sort_direct(&mut nodes);
                    black_box(nodes)
                },
                criterion::BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("schwartzian", size), |b| {
            b.iter_batched(
                || test_nodes.clone(),
                |mut nodes| {
                    sort_schwartzian(&mut nodes);
                    black_box(nodes)
                },
                criterion::BatchSize::SmallInput,
            )
        });

        group.bench_function(BenchmarkId::new("cached_key", size), |b| {
            b.iter_batched(
                || test_nodes.clone(),
                |mut nodes| {
                    sort_cached_key(&mut nodes);
                    black_box(nodes)
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

/// Micro-benchmark just the from_path function to understand its cost
fn bench_from_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("from_path cost");

    // Generate a variety of paths
    let paths: Vec<Nibbles> = (0..10_000)
        .map(|i| {
            let len = i % 65; // 0-64 nibbles
            Nibbles::from_nibbles_unchecked(vec![((i * 7) % 16) as u8; len])
        })
        .collect();

    group.bench_function("10k from_path calls", |b| {
        b.iter(|| {
            for path in &paths {
                black_box(SparseSubtrieType::from_path(path));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_sort_approaches, bench_from_path);
criterion_main!(benches);
