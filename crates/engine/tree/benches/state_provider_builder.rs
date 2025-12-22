//! Benchmark for state provider builder reuse optimization.
//!
//! Compares single lookup (reusing builder) vs double lookup (rebuilding).

#![allow(missing_docs)]

use alloy_primitives::B256;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
use reth_ethereum_primitives::EthPrimitives;
use std::sync::Arc;

/// Simulates the cost of building and cloning a state provider builder once (optimized path)
fn single_lookup(blocks: &[ExecutedBlock<EthPrimitives>]) -> Vec<ExecutedBlock<EthPrimitives>> {
    // Single lookup: clone once, use the result
    blocks.to_vec()
}

/// Simulates the cost of building twice (unoptimized path - what we eliminated)
fn double_lookup(
    blocks: &[ExecutedBlock<EthPrimitives>],
) -> (Vec<ExecutedBlock<EthPrimitives>>, Vec<ExecutedBlock<EthPrimitives>>) {
    // Double lookup: build Vec twice (simulating two blocks_by_hash calls)
    let first = blocks.to_vec();
    let second = blocks.to_vec();
    (first, second)
}

fn create_executed_blocks(count: usize) -> Vec<ExecutedBlock<EthPrimitives>> {
    let mut builder = TestBlockBuilder::eth();
    (0..count).map(|i| builder.get_executed_block_with_number(i as u64, B256::ZERO)).collect()
}

fn bench_state_provider_builder(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_provider_builder");

    // Test with realistic overlay sizes (blocks in memory)
    for size in [1, 10, 50, 100] {
        let blocks = create_executed_blocks(size);

        group.bench_with_input(BenchmarkId::new("single_lookup", size), &blocks, |b, blocks| {
            b.iter(|| black_box(single_lookup(blocks)))
        });

        group.bench_with_input(BenchmarkId::new("double_lookup", size), &blocks, |b, blocks| {
            b.iter(|| black_box(double_lookup(blocks)))
        });
    }

    group.finish();
}

criterion_group!(benches, bench_state_provider_builder);
criterion_main!(benches);
