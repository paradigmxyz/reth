//! Benchmark for cloning `Vec<ExecutedBlock>` to measure the cost of
//! `StateProviderBuilder` cloning.
//!
//! This benchmark helps quantify the performance impact of cloning the overlay
//! blocks when building state providers.

#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
use reth_ethereum_primitives::EthPrimitives;
use std::sync::Arc;

/// Create a vector of executed blocks for benchmarking
fn create_executed_blocks(count: usize) -> Vec<ExecutedBlock<EthPrimitives>> {
    let mut builder = TestBlockBuilder::eth();
    (0..count).map(|_| builder.get_executed_block_with_number(0, B256::ZERO)).collect()
}

use alloy_primitives::B256;

fn bench_vec_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("executed_block_clone");

    for size in [1, 10, 50, 100, 200] {
        let blocks = create_executed_blocks(size);

        group.throughput(Throughput::Elements(size as u64));

        // Benchmark cloning Vec<ExecutedBlock> directly (current approach)
        group.bench_with_input(BenchmarkId::new("vec_clone", size), &blocks, |b, blocks| {
            b.iter(|| black_box(blocks.clone()))
        });

        // Benchmark cloning Arc<Vec<ExecutedBlock>> (potential optimization)
        let arc_blocks = Arc::new(blocks.clone());
        group.bench_with_input(
            BenchmarkId::new("arc_vec_clone", size),
            &arc_blocks,
            |b, arc_blocks| b.iter(|| black_box(Arc::clone(arc_blocks))),
        );
    }

    group.finish();
}

criterion_group!(benches, bench_vec_clone);
criterion_main!(benches);
