//! Microbenchmarks for pruning algorithm variants.
//!
//! Run with: `cargo bench -p reth-trie-sparse-parallel --bench prune`

use alloy_primitives::{B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie_common::Nibbles;
use reth_trie_sparse::{provider::DefaultTrieNodeProvider, RevealableSparseTrie, SparseTrieExt};
use reth_trie_sparse_parallel::ParallelSparseTrie;
use std::collections::{HashMap, HashSet};

/// Helper to get a slice of nibbles as a new Nibbles
fn nibbles_prefix(n: &Nibbles, len: usize) -> Nibbles {
    n.slice(..len)
}

/// Helper to get nibbles as a byte slice for packing
fn nibbles_as_bytes(n: &Nibbles) -> impl Iterator<Item = u8> + '_ {
    (0..n.len()).map(|i| n.get_unchecked(i))
}

/// Generate random storage keys for benchmarking.
fn generate_test_data(size: usize) -> Vec<(B256, U256)> {
    let mut runner = TestRunner::deterministic();
    proptest::collection::vec(any::<(B256, U256)>(), size).new_tree(&mut runner).unwrap().current()
}

/// Build a revealed parallel sparse trie from test data.
fn build_trie(data: &[(B256, U256)]) -> RevealableSparseTrie<ParallelSparseTrie> {
    let provider = DefaultTrieNodeProvider;
    let mut trie = RevealableSparseTrie::<ParallelSparseTrie>::revealed_empty();

    for (key, value) in data {
        trie.update_leaf(
            Nibbles::unpack(key),
            alloy_rlp::encode_fixed_size(value).to_vec(),
            &provider,
        )
        .unwrap();
    }

    // Must compute root before pruning (nodes need hashes)
    trie.root().unwrap();
    trie
}

// ============================================================================
// ALGORITHM VARIANTS TO BENCHMARK
// ============================================================================

/// Current algorithm: sorted binary search + starts_with prefix check
fn descendant_check_binary_search(roots: &[Nibbles], paths: &[Nibbles]) -> usize {
    let mut sorted_roots = roots.to_vec();
    sorted_roots.sort_unstable();

    let starts_with_pruned = |p: &Nibbles| -> bool {
        let idx = sorted_roots.partition_point(|root| root <= p);
        if idx > 0 {
            let candidate = &sorted_roots[idx - 1];
            if p.starts_with(candidate) {
                return true;
            }
        }
        false
    };

    paths.iter().filter(|p| starts_with_pruned(p)).count()
}

/// Alternative 1: Bucketed HashSet by nibble length
fn descendant_check_bucketed_hashset(roots: &[Nibbles], paths: &[Nibbles]) -> usize {
    // Group roots by nibble length
    let mut buckets: HashMap<usize, HashSet<Nibbles>> = HashMap::new();
    for root in roots {
        buckets.entry(root.len()).or_default().insert(root.clone());
    }

    // Sorted lengths for consistent iteration
    let mut lengths: Vec<usize> = buckets.keys().copied().collect();
    lengths.sort_unstable();

    let starts_with_pruned = |p: &Nibbles| -> bool {
        for &len in &lengths {
            if p.len() >= len {
                let prefix = nibbles_prefix(p, len);
                if buckets.get(&len).map_or(false, |set| set.contains(&prefix)) {
                    return true;
                }
            }
        }
        false
    };

    paths.iter().filter(|p| starts_with_pruned(p)).count()
}

/// Alternative 2: Packed u64 bucketed (for tries where roots have similar nibble lengths)
fn descendant_check_packed_u64(roots: &[Nibbles], paths: &[Nibbles]) -> usize {
    // Pack nibbles into u64 (up to 16 nibbles = 64 bits)
    fn pack_nibbles(n: &Nibbles, len: usize) -> u64 {
        let mut packed = 0u64;
        for i in 0..len.min(16) {
            packed |= (n.get_unchecked(i) as u64 & 0xF) << (60 - i * 4);
        }
        packed
    }

    // Group by length, store packed values
    let mut buckets: HashMap<usize, HashSet<u64>> = HashMap::new();
    for root in roots {
        let packed = pack_nibbles(root, root.len());
        buckets.entry(root.len()).or_default().insert(packed);
    }

    let mut lengths: Vec<usize> = buckets.keys().copied().collect();
    lengths.sort_unstable();

    let starts_with_pruned = |p: &Nibbles| -> bool {
        for &len in &lengths {
            if p.len() >= len {
                let packed = pack_nibbles(p, len);
                if buckets.get(&len).map_or(false, |set| set.contains(&packed)) {
                    return true;
                }
            }
        }
        false
    };

    paths.iter().filter(|p| starts_with_pruned(p)).count()
}

/// Alternative 3: Single HashSet with all prefixes (trade memory for speed)
fn descendant_check_prefix_expansion(
    roots: &[Nibbles],
    paths: &[Nibbles],
    max_path_len: usize,
) -> usize {
    // Pre-expand all prefixes that would match
    let mut prefix_set: HashSet<Nibbles> = HashSet::new();
    for root in roots {
        // Add the root itself and mark that anything starting with it matches
        prefix_set.insert(root.clone());
    }

    // For exact prefix matching, we just check if any root is a prefix
    let starts_with_pruned = |p: &Nibbles| -> bool {
        // Check all possible prefix lengths
        for len in 1..=p.len().min(max_path_len) {
            let prefix = nibbles_prefix(p, len);
            if prefix_set.contains(&prefix) {
                return true;
            }
        }
        false
    };

    paths.iter().filter(|p| starts_with_pruned(p)).count()
}

// ============================================================================
// BENCHMARKS
// ============================================================================

fn bench_descendant_check_algorithms(c: &mut Criterion) {
    let mut group = c.benchmark_group("descendant_check");

    // Test different trie sizes
    for num_leaves in [1_000, 10_000, 50_000] {
        let data = generate_test_data(num_leaves);
        let trie = build_trie(&data);

        // Collect all paths from the trie for benchmarking
        let all_paths: Vec<Nibbles> = data.iter().map(|(k, _)| Nibbles::unpack(k)).collect();

        // Simulate prune roots at different depths
        for max_depth in [2, 3, 4] {
            // Generate synthetic prune roots (simulate what DFS would find)
            // In reality these come from the trie structure, but for benchmarking
            // we approximate with random prefixes
            let num_roots = 16usize.pow(max_depth as u32).min(1000);
            let prune_roots: Vec<Nibbles> = (0..num_roots)
                .map(|i| {
                    // Create paths of varying lengths (simulating extension nodes)
                    let base_len = max_depth * 2; // approximate nibbles per depth
                    let extra = i % 3; // vary length slightly
                    let path_data: Vec<u8> =
                        (0..(base_len + extra)).map(|j| ((i + j) % 16) as u8).collect();
                    Nibbles::from_nibbles(&path_data)
                })
                .collect();

            let id =
                format!("leaves={}/depth={}/roots={}", num_leaves, max_depth, prune_roots.len());
            group.throughput(Throughput::Elements(all_paths.len() as u64));

            // Benchmark current algorithm
            group.bench_function(BenchmarkId::new("binary_search", &id), |b| {
                b.iter(|| {
                    black_box(descendant_check_binary_search(
                        black_box(&prune_roots),
                        black_box(&all_paths),
                    ))
                })
            });

            // Benchmark bucketed hashset
            group.bench_function(BenchmarkId::new("bucketed_hashset", &id), |b| {
                b.iter(|| {
                    black_box(descendant_check_bucketed_hashset(
                        black_box(&prune_roots),
                        black_box(&all_paths),
                    ))
                })
            });

            // Benchmark packed u64
            group.bench_function(BenchmarkId::new("packed_u64", &id), |b| {
                b.iter(|| {
                    black_box(descendant_check_packed_u64(
                        black_box(&prune_roots),
                        black_box(&all_paths),
                    ))
                })
            });

            // Benchmark prefix expansion
            let max_path_len = all_paths.iter().map(|p| p.len()).max().unwrap_or(64);
            group.bench_function(BenchmarkId::new("prefix_expansion", &id), |b| {
                b.iter(|| {
                    black_box(descendant_check_prefix_expansion(
                        black_box(&prune_roots),
                        black_box(&all_paths),
                        max_path_len,
                    ))
                })
            });
        }
    }

    group.finish();
}

fn bench_full_prune(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_prune");
    group.sample_size(10); // Fewer samples for expensive operations

    for num_leaves in [1_000, 10_000, 50_000] {
        let data = generate_test_data(num_leaves);

        for max_depth in [2, 3, 4] {
            let id = format!("leaves={}/depth={}", num_leaves, max_depth);

            group.bench_function(BenchmarkId::new("current", &id), |b| {
                b.iter_with_setup(
                    || build_trie(&data),
                    |mut trie| {
                        let revealed = trie.as_revealed_mut().unwrap();
                        let pruned = revealed.prune(black_box(max_depth));
                        black_box(pruned)
                    },
                )
            });
        }
    }

    group.finish();
}

criterion_group!(prune_benches, bench_descendant_check_algorithms, bench_full_prune,);
criterion_main!(prune_benches);
