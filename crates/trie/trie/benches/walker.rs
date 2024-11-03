#![allow(missing_docs)]
use alloy_primitives::FixedBytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::prelude::*;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{prefix_set::{PrefixSet, PrefixSetMut}, trie_cursor::TrieCursor, walker::TrieWalker, BranchNodeCompact, Nibbles, TrieMask};
use std::time::Duration;

/// Test data structure to simulate a trie cursor
#[derive(Clone)]
struct TestCursor {
    depth: usize,
    branching_factor: usize,
    state_density: f64,
}

impl TrieCursor for TestCursor {
    fn seek(&mut self, _key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        // Simulate node lookup
        Ok(Some((
            Nibbles::from_vec(vec![1, 2, 3]),
            BranchNodeCompact {
                state_mask: TrieMask::new(0b1010), // Some arbitrary state
                tree_mask: TrieMask::new(0b1100),  // Some arbitrary tree state
                hash_mask: TrieMask::new(0b1000), 
                hashes: [FixedBytes::random()].to_vec(),
                root_hash: Some(FixedBytes::random()), // Some arbitrary hash state
            },
        )))
    }

    fn seek_exact(&mut self, key: Nibbles) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        self.seek(key)
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(Some(Nibbles::from_vec(vec![1, 2, 3])))
    }
    
    #[doc = " Move the cursor to the next key."]
    fn next(&mut self) -> Result<Option<(Nibbles,BranchNodeCompact)> ,DatabaseError>  {
        todo!()
    }
}

/// Generate test trie data with varying characteristics
fn generate_test_trie(depth: usize, branching_factor: usize, state_density: f64) -> TrieWalker<TestCursor> {
    let cursor = TestCursor {
        depth,
        branching_factor,
        state_density,
    };
    
    let mut changes = PrefixSetMut::default();
    // Add some test prefixes
    changes.insert(Nibbles::from_vec(vec![1, 2, 3]));
    
    TrieWalker::new(cursor, changes.freeze())
}

pub fn sibling_traversal_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("sibling_traversal");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    // Test cases with different trie characteristics
    let test_cases = vec![
        // (depth, branching_factor, state_density, name)
        (3, 4, 0.5, "small_sparse"),
        (5, 8, 0.7, "medium_dense"),
        (10, 16, 0.3, "deep_sparse"),
        (15, 4, 0.9, "very_deep_dense"),
    ];

    for (depth, branching, density, name) in test_cases {
        // Create test data
        let test_walker = generate_test_trie(depth, branching, density);
        
        // Benchmark recursive version
        group.bench_with_input(
            BenchmarkId::new("recursive", name), 
            &(depth, branching, density),
            |b, &(d, br, den)| {
                b.iter_batched(
                    || generate_test_trie(d, br, den),
                    |mut walker| {
                        black_box(walker.move_to_next_sibling(true)).unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );

        // Benchmark non-recursive version
        group.bench_with_input(
            BenchmarkId::new("non_recursive", name),
            &(depth, branching, density),
            |b, &(d, br, den)| {
                b.iter_batched(
                    || generate_test_trie(d, br, den),
                    |mut walker| {
                        black_box(walker.move_to_next_siblins(true)).unwrap();
                    },
                    criterion::BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}



criterion_group!(
    name = benches;
    config = Criterion::default()
        .with_plots()
        .sample_size(100)
        .measurement_time(Duration::from_secs(10));
    targets = sibling_traversal_benchmark
);
criterion_main!(benches);

// Additional stress test for deep tries
#[test]
fn stress_test_deep_trie() {
    let depths = [50, 100, 200, 500];
    
    for depth in depths {
        let mut walker = generate_test_trie(depth, 4, 0.5);
        
        // Test recursive version
        let recursive_start = std::time::Instant::now();
        walker.move_to_next_sibling(true).unwrap();
        let recursive_time = recursive_start.elapsed();

        // Reset walker
        let mut walker = generate_test_trie(depth, 4, 0.5);
        
        // Test non-recursive version
        let non_recursive_start = std::time::Instant::now();
        walker.move_to_next_siblins(true).unwrap();
        let non_recursive_time = non_recursive_start.elapsed();

        println!("Depth {}: Recursive: {:?}, Non-recursive: {:?}", 
                depth, recursive_time, non_recursive_time);
    }
}