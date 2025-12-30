#![allow(missing_docs, unreachable_pub)]
//! Benchmark comparing two representations for trie updates:
//! 1. Current: `HashMap<Nibbles, BranchNodeCompact>` + `HashSet<Nibbles>` (separate)
//! 2. Consolidated: `HashMap<Nibbles, Option<BranchNodeCompact>>` (unified)
//!
//! The consolidation aims to:
//! - Reduce `HashMap` overhead (one map instead of two)
//! - Improve memory layout (less fragmentation)
//! - Reduce cache misses (related data stored together)

use alloy_primitives::map::{HashMap, HashSet};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use prop::test_runner::TestRng;
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie_common::{BranchNodeCompact, Nibbles, TrieMask};
use std::hint::black_box;

/// Current representation: separate `HashMap` and `HashSet`
#[derive(Default, Clone)]
struct TrieUpdatesSeparate {
    account_nodes: HashMap<Nibbles, BranchNodeCompact>,
    removed_nodes: HashSet<Nibbles>,
}

impl TrieUpdatesSeparate {
    fn insert_node(&mut self, path: Nibbles, node: BranchNodeCompact) {
        self.removed_nodes.remove(&path);
        self.account_nodes.insert(path, node);
    }

    fn remove_node(&mut self, path: Nibbles) {
        self.account_nodes.remove(&path);
        self.removed_nodes.insert(path);
    }

    fn get_node(&self, path: &Nibbles) -> Option<&BranchNodeCompact> {
        self.account_nodes.get(path)
    }

    fn is_removed(&self, path: &Nibbles) -> bool {
        self.removed_nodes.contains(path)
    }

    fn is_empty(&self) -> bool {
        self.account_nodes.is_empty() && self.removed_nodes.is_empty()
    }

    fn len(&self) -> usize {
        self.account_nodes.len() + self.removed_nodes.len()
    }

    fn extend(&mut self, other: Self) {
        // Removed nodes in other should remove existing account nodes
        self.account_nodes.retain(|k, _| !other.removed_nodes.contains(k));
        self.account_nodes.extend(other.account_nodes);
        self.removed_nodes.extend(other.removed_nodes);
    }

    fn iter_all(&self) -> impl Iterator<Item = (&Nibbles, Option<&BranchNodeCompact>)> {
        self.account_nodes
            .iter()
            .map(|(k, v)| (k, Some(v)))
            .chain(self.removed_nodes.iter().map(|k| (k, None)))
    }
}

/// Consolidated representation: single `HashMap` with `Option`
#[derive(Default, Clone)]
struct TrieUpdatesConsolidated {
    /// None = removed, Some = updated
    nodes: HashMap<Nibbles, Option<BranchNodeCompact>>,
}

impl TrieUpdatesConsolidated {
    fn insert_node(&mut self, path: Nibbles, node: BranchNodeCompact) {
        self.nodes.insert(path, Some(node));
    }

    fn remove_node(&mut self, path: Nibbles) {
        self.nodes.insert(path, None);
    }

    fn get_node(&self, path: &Nibbles) -> Option<&BranchNodeCompact> {
        self.nodes.get(path).and_then(|opt| opt.as_ref())
    }

    fn is_removed(&self, path: &Nibbles) -> bool {
        self.nodes.get(path).is_some_and(|opt| opt.is_none())
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn extend(&mut self, other: Self) {
        // When extending, other's entries override ours
        // If other marks a path as removed (None), it overrides our entry
        // If other has a node (Some), it overrides our entry
        self.nodes.extend(other.nodes);
    }

    fn iter_all(&self) -> impl Iterator<Item = (&Nibbles, Option<&BranchNodeCompact>)> {
        self.nodes.iter().map(|(k, v)| (k, v.as_ref()))
    }
}

fn print_sizes() {
    println!("\n=== Type Sizes ===");
    println!("Nibbles: {} bytes", std::mem::size_of::<Nibbles>());
    println!("BranchNodeCompact: {} bytes", std::mem::size_of::<BranchNodeCompact>());
    println!(
        "Option<BranchNodeCompact>: {} bytes",
        std::mem::size_of::<Option<BranchNodeCompact>>()
    );
    println!(
        "(Nibbles, BranchNodeCompact): {} bytes",
        std::mem::size_of::<(Nibbles, BranchNodeCompact)>()
    );
    println!(
        "(Nibbles, Option<BranchNodeCompact>): {} bytes",
        std::mem::size_of::<(Nibbles, Option<BranchNodeCompact>)>()
    );
    println!();
}

fn generate_nibbles(runner: &mut TestRunner, count: usize) -> Vec<Nibbles> {
    use prop::collection::vec;
    let strategy = vec(any_with::<Nibbles>((1..=32usize).into()), count);
    let mut nibbles = strategy.new_tree(runner).unwrap().current();
    nibbles.sort();
    nibbles.dedup();
    nibbles
}

fn generate_branch_node() -> BranchNodeCompact {
    // Create a valid BranchNodeCompact with matching hash_mask and hashes count
    // hash_mask must have exactly the same number of bits set as hashes.len()
    BranchNodeCompact::new(
        TrieMask::new(0b1111_0000_1111_0000), // state_mask
        TrieMask::new(0b0011_0000_0011_0000), // tree_mask
        TrieMask::new(0),                     // hash_mask (0 bits = 0 hashes)
        vec![],                               // hashes (empty)
        None,                                 // root_hash
    )
}

fn bench_insert(c: &mut Criterion) {
    print_sizes();
    let mut group = c.benchmark_group("trie_updates_insert");

    for size in [100, 1_000, 10_000] {
        let config = proptest::test_runner::Config::default();
        let rng = TestRng::deterministic_rng(config.rng_algorithm);
        let mut runner = TestRunner::new_with_rng(config, rng);
        let nibbles = generate_nibbles(&mut runner, size);
        let node = generate_branch_node();

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("separate", size), &size, |b, _| {
            b.iter(|| {
                let mut updates = TrieUpdatesSeparate::default();
                for path in &nibbles {
                    updates.insert_node(*path, node.clone());
                }
                black_box(updates)
            });
        });

        group.bench_with_input(BenchmarkId::new("consolidated", size), &size, |b, _| {
            b.iter(|| {
                let mut updates = TrieUpdatesConsolidated::default();
                for path in &nibbles {
                    updates.insert_node(*path, node.clone());
                }
                black_box(updates)
            });
        });
    }

    group.finish();
}

fn bench_mixed_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie_updates_mixed");

    for size in [100, 1_000, 10_000] {
        let config = proptest::test_runner::Config::default();
        let rng = TestRng::deterministic_rng(config.rng_algorithm);
        let mut runner = TestRunner::new_with_rng(config, rng);
        let nibbles = generate_nibbles(&mut runner, size);
        let node = generate_branch_node();

        group.throughput(Throughput::Elements(size as u64));

        // Mix of 70% inserts, 30% removes
        group.bench_with_input(BenchmarkId::new("separate", size), &size, |b, _| {
            b.iter(|| {
                let mut updates = TrieUpdatesSeparate::default();
                for (i, path) in nibbles.iter().enumerate() {
                    if i % 10 < 7 {
                        updates.insert_node(*path, node.clone());
                    } else {
                        updates.remove_node(*path);
                    }
                }
                black_box(updates)
            });
        });

        group.bench_with_input(BenchmarkId::new("consolidated", size), &size, |b, _| {
            b.iter(|| {
                let mut updates = TrieUpdatesConsolidated::default();
                for (i, path) in nibbles.iter().enumerate() {
                    if i % 10 < 7 {
                        updates.insert_node(*path, node.clone());
                    } else {
                        updates.remove_node(*path);
                    }
                }
                black_box(updates)
            });
        });
    }

    group.finish();
}

fn bench_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie_updates_lookup");

    for size in [100, 1_000, 10_000] {
        let config = proptest::test_runner::Config::default();
        let rng = TestRng::deterministic_rng(config.rng_algorithm);
        let mut runner = TestRunner::new_with_rng(config, rng);
        let nibbles = generate_nibbles(&mut runner, size);
        let node = generate_branch_node();

        // Pre-populate the structures
        let mut separate = TrieUpdatesSeparate::default();
        let mut consolidated = TrieUpdatesConsolidated::default();
        for (i, path) in nibbles.iter().enumerate() {
            if i % 10 < 7 {
                separate.insert_node(*path, node.clone());
                consolidated.insert_node(*path, node.clone());
            } else {
                separate.remove_node(*path);
                consolidated.remove_node(*path);
            }
        }

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("separate", size), &size, |b, _| {
            b.iter(|| {
                let mut found = 0usize;
                for path in &nibbles {
                    if separate.get_node(path).is_some() {
                        found += 1;
                    }
                    if separate.is_removed(path) {
                        found += 1;
                    }
                }
                black_box(found)
            });
        });

        group.bench_with_input(BenchmarkId::new("consolidated", size), &size, |b, _| {
            b.iter(|| {
                let mut found = 0usize;
                for path in &nibbles {
                    if consolidated.get_node(path).is_some() {
                        found += 1;
                    }
                    if consolidated.is_removed(path) {
                        found += 1;
                    }
                }
                black_box(found)
            });
        });
    }

    group.finish();
}

fn bench_extend(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie_updates_extend");

    for size in [100, 1_000, 10_000] {
        let config = proptest::test_runner::Config::default();
        let rng = TestRng::deterministic_rng(config.rng_algorithm);
        let mut runner = TestRunner::new_with_rng(config, rng);
        let nibbles1 = generate_nibbles(&mut runner, size);
        let nibbles2 = generate_nibbles(&mut runner, size);
        let node = generate_branch_node();

        // Pre-populate first set
        let mut separate1 = TrieUpdatesSeparate::default();
        let mut consolidated1 = TrieUpdatesConsolidated::default();
        for (i, path) in nibbles1.iter().enumerate() {
            if i % 10 < 7 {
                separate1.insert_node(*path, node.clone());
                consolidated1.insert_node(*path, node.clone());
            } else {
                separate1.remove_node(*path);
                consolidated1.remove_node(*path);
            }
        }

        // Pre-populate second set
        let mut separate2 = TrieUpdatesSeparate::default();
        let mut consolidated2 = TrieUpdatesConsolidated::default();
        for (i, path) in nibbles2.iter().enumerate() {
            if i % 10 < 7 {
                separate2.insert_node(*path, node.clone());
                consolidated2.insert_node(*path, node.clone());
            } else {
                separate2.remove_node(*path);
                consolidated2.remove_node(*path);
            }
        }

        group.throughput(Throughput::Elements((size * 2) as u64));

        group.bench_with_input(BenchmarkId::new("separate", size), &size, |b, _| {
            b.iter_batched(
                || (separate1.clone(), separate2.clone()),
                |(mut s1, s2)| {
                    s1.extend(s2);
                    black_box(s1)
                },
                criterion::BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("consolidated", size), &size, |b, _| {
            b.iter_batched(
                || (consolidated1.clone(), consolidated2.clone()),
                |(mut c1, c2)| {
                    c1.extend(c2);
                    black_box(c1)
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_iteration(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie_updates_iteration");

    for size in [100, 1_000, 10_000] {
        let config = proptest::test_runner::Config::default();
        let rng = TestRng::deterministic_rng(config.rng_algorithm);
        let mut runner = TestRunner::new_with_rng(config, rng);
        let nibbles = generate_nibbles(&mut runner, size);
        let node = generate_branch_node();

        // Pre-populate the structures
        let mut separate = TrieUpdatesSeparate::default();
        let mut consolidated = TrieUpdatesConsolidated::default();
        for (i, path) in nibbles.iter().enumerate() {
            if i % 10 < 7 {
                separate.insert_node(*path, node.clone());
                consolidated.insert_node(*path, node.clone());
            } else {
                separate.remove_node(*path);
                consolidated.remove_node(*path);
            }
        }

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("separate", size), &size, |b, _| {
            b.iter(|| {
                let mut count = 0usize;
                for (_, node) in separate.iter_all() {
                    if node.is_some() {
                        count += 1;
                    }
                }
                black_box(count)
            });
        });

        group.bench_with_input(BenchmarkId::new("consolidated", size), &size, |b, _| {
            b.iter(|| {
                let mut count = 0usize;
                for (_, node) in consolidated.iter_all() {
                    if node.is_some() {
                        count += 1;
                    }
                }
                black_box(count)
            });
        });
    }

    group.finish();
}

fn bench_memory_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie_updates_memory");

    for size in [100, 1_000, 10_000] {
        let config = proptest::test_runner::Config::default();
        let rng = TestRng::deterministic_rng(config.rng_algorithm);
        let mut runner = TestRunner::new_with_rng(config, rng);
        let nibbles = generate_nibbles(&mut runner, size);
        let node = generate_branch_node();

        // Pre-populate with 70% inserts, 30% removes
        let mut separate = TrieUpdatesSeparate::default();
        let mut consolidated = TrieUpdatesConsolidated::default();
        for (i, path) in nibbles.iter().enumerate() {
            if i % 10 < 7 {
                separate.insert_node(*path, node.clone());
                consolidated.insert_node(*path, node.clone());
            } else {
                separate.remove_node(*path);
                consolidated.remove_node(*path);
            }
        }

        // Calculate approximate memory usage
        let separate_size = std::mem::size_of::<TrieUpdatesSeparate>() +
            separate.account_nodes.capacity() *
                (std::mem::size_of::<Nibbles>() + std::mem::size_of::<BranchNodeCompact>()) +
            separate.removed_nodes.capacity() * std::mem::size_of::<Nibbles>();

        let consolidated_size = std::mem::size_of::<TrieUpdatesConsolidated>() +
            consolidated.nodes.capacity() *
                (std::mem::size_of::<Nibbles>() +
                    std::mem::size_of::<Option<BranchNodeCompact>>());

        println!(
            "Size {}: Separate={} bytes, Consolidated={} bytes, Savings={}%",
            size,
            separate_size,
            consolidated_size,
            100 - (consolidated_size * 100 / separate_size.max(1))
        );

        // Benchmark memory allocation overhead
        group.bench_with_input(BenchmarkId::new("separate_alloc", size), &size, |b, _| {
            b.iter(|| {
                let mut updates = TrieUpdatesSeparate::default();
                for (i, path) in nibbles.iter().enumerate() {
                    if i % 10 < 7 {
                        updates.insert_node(*path, node.clone());
                    } else {
                        updates.remove_node(*path);
                    }
                }
                black_box((updates.len(), updates.is_empty()))
            });
        });

        group.bench_with_input(BenchmarkId::new("consolidated_alloc", size), &size, |b, _| {
            b.iter(|| {
                let mut updates = TrieUpdatesConsolidated::default();
                for (i, path) in nibbles.iter().enumerate() {
                    if i % 10 < 7 {
                        updates.insert_node(*path, node.clone());
                    } else {
                        updates.remove_node(*path);
                    }
                }
                black_box((updates.len(), updates.is_empty()))
            });
        });
    }

    group.finish();
}

criterion_group!(
    trie_updates,
    bench_insert,
    bench_mixed_operations,
    bench_lookup,
    bench_extend,
    bench_iteration,
    bench_memory_size,
);
criterion_main!(trie_updates);
