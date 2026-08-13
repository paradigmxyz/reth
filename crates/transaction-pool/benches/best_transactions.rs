#![allow(missing_docs)]

//! Benchmarks for creating and consuming best transaction snapshots.

use std::{hint::black_box, sync::Arc};

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use reth_transaction_pool::{
    pool::PendingPool,
    test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
    BestTransactions,
};

const TRANSACTION_COUNTS: &[usize] = &[1_000, 10_000, 100_000];

fn pool(transaction_count: usize, sender_count: usize) -> PendingPool<MockOrdering> {
    let mut pool = PendingPool::new(MockOrdering::default());
    let mut factory = MockTransactionFactory::default();
    for index in 0..transaction_count {
        let sender = index % sender_count;
        let transaction = MockTransaction::eip1559()
            .with_sender(Address::from_word(U256::from(sender + 1).into()))
            .with_nonce((index / sender_count) as u64)
            .with_hash(B256::from(U256::from(index + 1)));
        pool.add_transaction(Arc::new(factory.validated(transaction)), 0);
    }
    pool
}

fn best_transactions(c: &mut Criterion) {
    let mut group = c.benchmark_group("best_transactions");
    group.sample_size(10);

    for &transaction_count in TRANSACTION_COUNTS {
        let pool = pool(transaction_count, transaction_count);
        group.throughput(Throughput::Elements(transaction_count as u64));

        group.bench_with_input(
            BenchmarkId::new("end_to_end", transaction_count),
            &transaction_count,
            |b, _| {
                b.iter(|| {
                    let mut best = pool.best();
                    best.no_updates();
                    assert_eq!(best.by_ref().map(black_box).count(), transaction_count);
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("snapshot", transaction_count),
            &transaction_count,
            |b, _| {
                b.iter(|| {
                    let mut best = pool.best();
                    best.no_updates();
                    black_box(best);
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("iterate", transaction_count),
            &transaction_count,
            |b, _| {
                b.iter_batched(
                    || {
                        let mut best = pool.best();
                        best.no_updates();
                        best
                    },
                    |mut best| {
                        assert_eq!(best.by_ref().map(black_box).count(), transaction_count);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();

    for sender_count in [1, 1_000] {
        let pool = pool(100_000, sender_count);
        let mut group = c.benchmark_group(format!("best_transactions/{sender_count}_senders"));
        group.sample_size(10);
        group.throughput(Throughput::Elements(100_000));
        group.bench_function("end_to_end/100000", |b| {
            b.iter(|| {
                let mut best = pool.best();
                best.no_updates();
                assert_eq!(best.by_ref().map(black_box).count(), 100_000);
            });
        });
        group.finish();
    }
}

criterion_group!(benches, best_transactions);
criterion_main!(benches);
