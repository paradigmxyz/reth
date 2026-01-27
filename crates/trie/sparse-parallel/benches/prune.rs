#![allow(missing_docs)]

use alloy_primitives::U256;
use alloy_rlp::Encodable;
use alloy_trie::EMPTY_ROOT_HASH;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_primitives_traits::Account;
use reth_trie_common::Nibbles;
use reth_trie_sparse::{provider::DefaultTrieNodeProvider, SparseTrie, SparseTrieExt};
use reth_trie_sparse_parallel::{ParallelSparseTrie, ParallelismThresholds};

fn large_account_value() -> Vec<u8> {
    let account = Account {
        nonce: 0x123456789abcdef,
        balance: U256::from(0x123456789abcdef0123456789abcdef_u128),
        ..Default::default()
    };
    let mut buf = Vec::new();
    account.into_trie_account(EMPTY_ROOT_HASH).encode(&mut buf);
    buf
}

/// Build a trie with `num_lower_subtries` lower subtries revealed.
/// Each subtrie gets 4 leaves to ensure it's not trivially small.
fn build_trie_with_subtrie_count(num_lower_subtries: usize) -> ParallelSparseTrie {
    build_trie_with_subtrie_count_and_threshold(num_lower_subtries, 4)
}

fn build_trie_with_subtrie_count_and_threshold(
    num_lower_subtries: usize,
    min_prune_subtries: usize,
) -> ParallelSparseTrie {
    let provider = DefaultTrieNodeProvider;
    let value = large_account_value();

    let thresholds =
        ParallelismThresholds { min_revealed_nodes: 0, min_updated_nodes: 0, min_prune_subtries };

    let mut trie = ParallelSparseTrie::default().with_parallelism_thresholds(thresholds);

    for subtrie_idx in 0..num_lower_subtries {
        let first_nibble = (subtrie_idx / 16) as u8;
        let second_nibble = (subtrie_idx % 16) as u8;

        for leaf_idx in 0..4u8 {
            let path = Nibbles::from_nibbles([
                first_nibble,
                second_nibble,
                leaf_idx,
                0x3,
                0x4,
                0x5,
                0x6,
                0x7,
            ]);
            trie.update_leaf(path, value.clone(), &provider).unwrap();
        }
    }

    trie.root();
    trie
}

fn bench_prune_by_subtrie_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_by_subtrie_count");
    group.sample_size(100);

    for num_subtries in [2, 4, 8, 16, 32, 64, 128, 256] {
        let base_trie = build_trie_with_subtrie_count(num_subtries);

        group.bench_function(BenchmarkId::new("subtries", num_subtries), |b| {
            b.iter_with_setup(
                || base_trie.clone(),
                |mut trie| {
                    let pruned = trie.prune(2);
                    black_box(pruned);
                    trie
                },
            )
        });
    }
    group.finish();
}

fn bench_prune_serial_vs_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_serial_vs_parallel");
    group.sample_size(200);

    for num_subtries in [4, 8, 16, 32, 64] {
        // Force serial: set threshold very high (usize::MAX means always serial)
        let serial_trie = build_trie_with_subtrie_count_and_threshold(num_subtries, usize::MAX);
        // Force parallel: set threshold to 1 (always parallel if >= 1 subtrie)
        let parallel_trie = build_trie_with_subtrie_count_and_threshold(num_subtries, 1);

        group.bench_function(BenchmarkId::new("serial", num_subtries), |b| {
            b.iter_with_setup(
                || serial_trie.clone(),
                |mut trie| {
                    let pruned = trie.prune(2);
                    black_box(pruned);
                    trie
                },
            )
        });

        group.bench_function(BenchmarkId::new("parallel", num_subtries), |b| {
            b.iter_with_setup(
                || parallel_trie.clone(),
                |mut trie| {
                    let pruned = trie.prune(2);
                    black_box(pruned);
                    trie
                },
            )
        });
    }
    group.finish();
}

fn bench_threshold_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("threshold_sweep");
    group.sample_size(200);

    // Test with 32 subtries - enough to see the difference
    let num_subtries = 32;

    for threshold in [1, 2, 4, 8, 16, 32, 64, usize::MAX] {
        let base_trie = build_trie_with_subtrie_count_and_threshold(num_subtries, threshold);
        let label = if threshold == usize::MAX {
            "serial".to_string()
        } else {
            format!("threshold_{threshold}")
        };

        group.bench_function(BenchmarkId::new("32_subtries", label), |b| {
            b.iter_with_setup(
                || base_trie.clone(),
                |mut trie| {
                    let pruned = trie.prune(2);
                    black_box(pruned);
                    trie
                },
            )
        });
    }
    group.finish();
}

fn bench_prune_depths(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_depths");
    group.sample_size(50);

    let base_trie = build_trie_with_subtrie_count(64);

    for depth in [0, 1, 2, 3, 4] {
        group.bench_function(BenchmarkId::new("depth", depth), |b| {
            b.iter_with_setup(
                || base_trie.clone(),
                |mut trie| {
                    let pruned = trie.prune(depth);
                    black_box(pruned);
                    trie
                },
            )
        });
    }
    group.finish();
}

fn bench_retain_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("retain_overhead");
    group.sample_size(100);

    let provider = DefaultTrieNodeProvider;
    let value = large_account_value();

    for nodes_per_subtrie in [4, 16, 64, 256] {
        let mut trie = ParallelSparseTrie::default();
        for subtrie_idx in 0..64usize {
            let first_nibble = (subtrie_idx / 16) as u8;
            let second_nibble = (subtrie_idx % 16) as u8;

            for leaf_idx in 0..nodes_per_subtrie as u16 {
                let third = (leaf_idx / 16) as u8;
                let fourth = (leaf_idx % 16) as u8;
                let path = Nibbles::from_nibbles([
                    first_nibble,
                    second_nibble,
                    third,
                    fourth,
                    0x5,
                    0x6,
                    0x7,
                    0x8,
                ]);
                trie.update_leaf(path, value.clone(), &provider).unwrap();
            }
        }
        trie.root();

        group.bench_function(BenchmarkId::new("nodes_per_subtrie", nodes_per_subtrie), |b| {
            b.iter_with_setup(
                || trie.clone(),
                |mut t| {
                    let pruned = t.prune(2);
                    black_box(pruned);
                    t
                },
            )
        });
    }
    group.finish();
}

criterion_group!(
    prune,
    bench_prune_by_subtrie_count,
    bench_prune_serial_vs_parallel,
    bench_threshold_sweep,
    bench_prune_depths,
    bench_retain_overhead,
);
criterion_main!(prune);
