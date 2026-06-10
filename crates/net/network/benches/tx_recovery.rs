#![allow(missing_docs)]

//! Compares strategies for recovering the signers of a batch of incoming transactions, as done
//! by the `TransactionsManager` when importing transactions received over the network.
//!
//! For profiling a single variant with samply:
//!
//! ```sh
//! cargo bench --bench tx_recovery -- --profile-time 10 "parallel/per-item/256"
//! samply record -p <pid>
//! ```

use alloy_consensus::transaction::SignerRecoverable;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use reth_ethereum_primitives::TransactionSigned;
use reth_transaction_pool::test_utils::TransactionGenerator;

/// Minimum work unit sizes when splitting a batch across the rayon pool.
const MIN_SPLIT_LENS: [usize; 2] = [8, 16];

fn generate_txs(count: usize) -> Vec<TransactionSigned> {
    let mut tx_gen = TransactionGenerator::with_num_signers(rand::rng(), count.max(10));
    (0..count).map(|_| tx_gen.gen_eip1559()).collect()
}

fn bench_recovery(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch tx recovery");

    for &batch_size in &[1usize, 4, 16, 64, 256] {
        let txs = generate_txs(batch_size);
        group.throughput(Throughput::Elements(batch_size as u64));

        group.bench_with_input(BenchmarkId::new("sequential", batch_size), &txs, |b, txs| {
            b.iter_batched(
                || txs.clone(),
                |txs| {
                    txs.into_iter()
                        .filter_map(|tx| tx.try_into_recovered().ok())
                        .collect::<Vec<_>>()
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_with_input(BenchmarkId::new("sequential/buf", batch_size), &txs, |b, txs| {
            let mut buf = Vec::new();
            b.iter_batched(
                || txs.clone(),
                |txs| {
                    txs.into_iter()
                        .filter_map(|tx| tx.try_into_recovered_with_buf(&mut buf).ok())
                        .collect::<Vec<_>>()
                },
                BatchSize::SmallInput,
            )
        });

        group.bench_with_input(
            BenchmarkId::new("parallel/per-item", batch_size),
            &txs,
            |b, txs| {
                b.iter_batched(
                    || txs.clone(),
                    |txs| {
                        txs.into_par_iter()
                            .filter_map(|tx| tx.try_into_recovered().ok())
                            .collect::<Vec<_>>()
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        for min_len in MIN_SPLIT_LENS {
            group.bench_with_input(
                BenchmarkId::new(format!("parallel/min-len-{min_len}/buf"), batch_size),
                &txs,
                |b, txs| {
                    b.iter_batched(
                        || txs.clone(),
                        |txs| {
                            txs.into_par_iter()
                                .with_min_len(min_len)
                                .map_init(Vec::new, |buf, tx| {
                                    tx.try_into_recovered_with_buf(buf).ok()
                                })
                                .filter_map(|tx| tx)
                                .collect::<Vec<_>>()
                        },
                        BatchSize::SmallInput,
                    )
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_recovery);
criterion_main!(benches);
