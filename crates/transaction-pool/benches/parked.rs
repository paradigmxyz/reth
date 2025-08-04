#![allow(missing_docs)]
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::Arc;

use reth_transaction_pool::{
    SubPoolLimit, ValidPoolTransaction,
    identifier::SenderId,
    pool::{BasefeeOrd, ParkedPool},
    test_utils::{MockTransaction, MockTransactionFactory, MockTransactionSet},
};

fn create_pool_with_txs(
    tx_count: usize,
    sender_count: usize,
) -> (ParkedPool<BasefeeOrd<MockTransaction>>, MockTransactionFactory) {
    let mut f = MockTransactionFactory::default();
    let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
    
    let txs_per_sender = tx_count / sender_count;
    
    for sender_idx in 0..sender_count {
        let sender_addr = format!("0x{:040x}", sender_idx).parse().unwrap();
        for nonce in 0..txs_per_sender {
            let tx = f.validated_arc(
                MockTransaction::eip1559()
                    .inc_price()
                    .rng_hash()
                    .with_sender(sender_addr)
                    .with_nonce(nonce as u64),
            );
            pool.add_transaction(tx);
        }
    }
    
    (pool, f)
}

fn bench_add_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_transaction");
    
    for &pool_size in &[100, 1000, 5000, 10000] {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("single_tx", pool_size),
            &pool_size,
            |b, &pool_size| {
                b.iter_batched(
                    || {
                        let (pool, mut f) = create_pool_with_txs(pool_size, pool_size / 10);
                        let new_tx = f.validated_arc(
                            MockTransaction::eip1559()
                                .inc_price()
                                .rng_hash()
                                .with_nonce(999999)
                        );
                        (pool, new_tx)
                    },
                    |(mut pool, tx)| {
                        pool.add_transaction(black_box(tx));
                        black_box(pool)
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_bulk_add_transactions(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_add_transactions");
    
    for &batch_size in &[10, 50, 100, 500] {
        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::new("batch", batch_size),
            &batch_size,
            |b, &batch_size| {
                b.iter_batched(
                    || {
                        let mut f = MockTransactionFactory::default();
                        let pool = ParkedPool::<BasefeeOrd<_>>::default();
                        let txs: Vec<_> = (0..batch_size)
                            .map(|nonce| {
                                f.validated_arc(
                                    MockTransaction::eip1559()
                                        .inc_price()
                                        .rng_hash()
                                        .with_nonce(nonce as u64)
                                )
                            })
                            .collect();
                        (pool, txs)
                    },
                    |(mut pool, txs)| {
                        for tx in txs {
                            pool.add_transaction(black_box(tx));
                        }
                        black_box(pool)
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}


fn bench_truncate_pool(c: &mut Criterion) {
    let mut group = c.benchmark_group("truncate_pool");
    
    for &pool_size in &[1000, 5000, 10000] {
        for &sender_count in &[10, 50, 100] {
            group.throughput(Throughput::Elements(pool_size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}txs_{}senders", pool_size, sender_count), pool_size),
                &(pool_size, sender_count),
                |b, &(pool_size, sender_count)| {
                    b.iter_batched(
                        || {
                            let (pool, _) = create_pool_with_txs(pool_size, sender_count);
                            let limit = SubPoolLimit {
                                max_txs: pool_size / 2,
                                max_size: usize::MAX,
                            };
                            (pool, limit)
                        },
                        |(mut pool, limit)| {
                            black_box(pool.truncate_pool(limit))
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    group.finish();
}


fn bench_scalability(c: &mut Criterion) {
    let mut group = c.benchmark_group("scalability");
    
    let pool_size = 5000;
    for &ratio in &[1, 10, 50, 100, 500] { 
        let sender_count = pool_size / ratio;
        group.throughput(Throughput::Elements(pool_size as u64));
        group.bench_with_input(
            BenchmarkId::new(format!("{}txs_per_sender", ratio), ratio),
            &(pool_size, sender_count),
            |b, &(pool_size, sender_count)| {
                b.iter(|| {
                    let (pool, _) = create_pool_with_txs(pool_size, sender_count);
                    black_box(pool)
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_add_transaction,
    bench_bulk_add_transactions,
    bench_truncate_pool,
    bench_scalability
);

criterion_main!(benches);