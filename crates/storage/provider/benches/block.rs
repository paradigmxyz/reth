#![allow(missing_docs, unreachable_pub)]
//! This benchmarks block insertion in the block provider

use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion,
    Throughput, black_box
};
use pprof::criterion::{Output, PProfProfiler};
use prop::test_runner::TestRunner;
use proptest::{prelude::*, strategy::ValueTree};
use reth_provider::{test_utils::create_test_provider_factory, BlockWriter};
use reth_primitives::{Address, PruneModes, SealedBlock, SealedBlockWithSenders};

fn block_provider_insert(c: &mut Criterion) {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    let mut group = c.benchmark_group("Block provider insert");

    // Configure the proptest runner
    let mut runner = TestRunner::new(ProptestConfig::default());

    let block_strategy = proptest::arbitrary::any::<SealedBlock>().prop_flat_map(|sealed_block| {
        proptest::collection::vec(proptest::arbitrary::any::<Address>(), sealed_block.body.len()).prop_map(move |senders| {
            SealedBlockWithSenders::new(sealed_block.clone(), senders).unwrap()
        })
    });

    // We can test different amounts of blocks
    let block_amounts = [10, 50, 100];

    for &size in &block_amounts {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("Insert", size), &size, |b, &size| {
            let blocks: Vec<SealedBlockWithSenders> = prop::collection::vec(block_strategy.clone(), size)
                .new_tree(&mut runner)
                .unwrap()
                .current();

            b.iter(|| {
                for block in &blocks {
                    let _result = provider.insert_block(black_box(block.clone()), None).unwrap();
                }
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = block_provider;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = block_provider_insert
}
criterion_main!(block_provider);
