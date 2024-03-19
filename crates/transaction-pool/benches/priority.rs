#![allow(missing_docs)]
use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_transaction_pool::{blob_tx_priority, fee_delta};

fn generate_test_data_fee_delta() -> (u128, u128) {
    let config = ProptestConfig::default();
    let mut runner = TestRunner::new(config);
    prop::arbitrary::any::<(u128, u128)>().new_tree(&mut runner).unwrap().current()
}

fn generate_test_data_priority() -> (u128, u128, u128, u128) {
    let config = ProptestConfig::default();
    let mut runner = TestRunner::new(config);
    prop::arbitrary::any::<(u128, u128, u128, u128)>().new_tree(&mut runner).unwrap().current()
}

fn priority_bench(
    group: &mut BenchmarkGroup<'_, WallTime>,
    description: &str,
    input_data: (u128, u128, u128, u128),
) {
    let group_id = format!("txpool | {description}");

    group.bench_function(group_id, |b| {
        b.iter(|| {
            black_box(blob_tx_priority(
                black_box(input_data.0),
                black_box(input_data.1),
                black_box(input_data.2),
                black_box(input_data.3),
            ));
        });
    });
}

fn fee_jump_bench(
    group: &mut BenchmarkGroup<'_, WallTime>,
    description: &str,
    input_data: (u128, u128),
) {
    let group_id = format!("txpool | {description}");

    group.bench_function(group_id, |b| {
        b.iter(|| {
            black_box(fee_delta(black_box(input_data.0), black_box(input_data.1)));
        });
    });
}

fn blob_priority_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Blob priority calculation");
    let fee_jump_input = generate_test_data_fee_delta();

    // Unstable sorting of unsorted collection
    fee_jump_bench(&mut group, "BenchmarkDynamicFeeJumpCalculation", fee_jump_input);

    let blob_priority_input = generate_test_data_priority();

    // BinaryHeap that is resorted on each update
    priority_bench(&mut group, "BenchmarkPriorityCalculation", blob_priority_input);
}

criterion_group!(priority, blob_priority_calculation);
criterion_main!(priority);
