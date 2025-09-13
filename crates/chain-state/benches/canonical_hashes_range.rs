#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reth_chain_state::{
    test_utils::TestBlockBuilder, ExecutedBlockWithTrieUpdates, MemoryOverlayStateProviderRef,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_storage_api::{noop::NoopProvider, BlockHashReader};

criterion_group!(benches, bench_canonical_hashes_range);
criterion_main!(benches);

fn bench_canonical_hashes_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_hashes_range");

    let scenarios = [("small", 10), ("medium", 100), ("large", 1000)];

    for (name, num_blocks) in scenarios {
        group.bench_function(format!("{}_blocks_{}", name, num_blocks), |b| {
            let (provider, blocks) = setup_provider_with_blocks(num_blocks);
            let start_block = blocks[0].recovered_block().number;
            let end_block = blocks[num_blocks / 2].recovered_block().number;

            b.iter(|| {
                black_box(
                    provider
                        .canonical_hashes_range(black_box(start_block), black_box(end_block))
                        .unwrap(),
                )
            })
        });
    }

    let (provider, blocks) = setup_provider_with_blocks(500);
    let base_block = blocks[100].recovered_block().number;

    let range_sizes = [1, 10, 50, 100, 250];
    for range_size in range_sizes {
        group.bench_function(format!("range_size_{}", range_size), |b| {
            let end_block = base_block + range_size;

            b.iter(|| {
                black_box(
                    provider
                        .canonical_hashes_range(black_box(base_block), black_box(end_block))
                        .unwrap(),
                )
            })
        });
    }

    // Benchmark edge cases
    group.bench_function("no_in_memory_matches", |b| {
        let (provider, blocks) = setup_provider_with_blocks(100);
        let first_block = blocks[0].recovered_block().number;
        let start_block = first_block - 50;
        let end_block = first_block - 10;

        b.iter(|| {
            black_box(
                provider
                    .canonical_hashes_range(black_box(start_block), black_box(end_block))
                    .unwrap(),
            )
        })
    });

    group.bench_function("all_in_memory_matches", |b| {
        let (provider, blocks) = setup_provider_with_blocks(100);
        let first_block = blocks[0].recovered_block().number;
        let last_block = blocks[blocks.len() - 1].recovered_block().number;

        b.iter(|| {
            black_box(
                provider
                    .canonical_hashes_range(black_box(first_block), black_box(last_block + 1))
                    .unwrap(),
            )
        })
    });

    group.finish();
}

fn setup_provider_with_blocks(
    num_blocks: usize,
) -> (
    MemoryOverlayStateProviderRef<'static, EthPrimitives>,
    Vec<ExecutedBlockWithTrieUpdates<EthPrimitives>>,
) {
    let mut builder = TestBlockBuilder::<EthPrimitives>::default();

    let blocks: Vec<_> = builder.get_executed_blocks(1000..1000 + num_blocks as u64).collect();

    let historical = Box::new(NoopProvider::default());
    let provider = MemoryOverlayStateProviderRef::new(historical, blocks.clone());

    (provider, blocks)
}
