//! Benchmark for state provider builder reuse.

#![allow(missing_docs)]

use alloy_consensus::Header;
use alloy_primitives::B256;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_engine_tree::tree::StateProviderBuilder;
use reth_ethereum_primitives::EthPrimitives;
use reth_provider::{test_utils::MockEthProvider, HeaderProvider};
use std::hint::black_box;

fn build_builder(
    provider: &MockEthProvider<EthPrimitives>,
    hash: B256,
) -> StateProviderBuilder<EthPrimitives, MockEthProvider<EthPrimitives>> {
    let header = provider.header(hash).expect("header lookup failed");
    assert!(header.is_some(), "missing header for hash");
    StateProviderBuilder::new(provider.clone(), hash, None)
}

fn bench_state_provider_builder(c: &mut Criterion) {
    let provider = MockEthProvider::<EthPrimitives>::new();
    let hash = B256::from([0x11u8; 32]);
    let header = Header { number: 1, ..Header::default() };
    provider.add_header(hash, header);

    let mut group = c.benchmark_group("state_provider_builder_reuse");

    group.bench_with_input(BenchmarkId::new("single_lookup", 1), &hash, |b, hash| {
        b.iter(|| {
            let builder = build_builder(&provider, *hash);
            black_box(builder);
        });
    });

    group.bench_with_input(BenchmarkId::new("double_lookup", 2), &hash, |b, hash| {
        b.iter(|| {
            let first = build_builder(&provider, *hash);
            let second = build_builder(&provider, *hash);
            black_box((first, second));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_state_provider_builder);
criterion_main!(benches);
