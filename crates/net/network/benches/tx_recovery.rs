#![allow(missing_docs)]

//! Compares signer recovery strategies for batches of full transactions received by the
//! `TransactionsManager`.

use alloy_consensus::transaction::SignerRecoverable;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use reth_ethereum_primitives::PooledTransactionVariant;
use reth_primitives_traits::SignedTransaction;
use reth_transaction_pool::{
    test_utils::TransactionGenerator, EthPooledTransaction, PoolTransaction,
};

const BATCH_SIZES: [usize; 7] = [1, 4, 16, 64, 256, 1024, 4096];
const MIN_PARALLEL_SPLIT_LENS: [usize; 4] = [8, 16, 32, 64];
const DEFAULT_MIN_PARALLEL_BATCH: usize = 16;

fn generate_txs(count: usize) -> Vec<PooledTransactionVariant> {
    let mut tx_gen = TransactionGenerator::with_num_signers(rand::rng(), count.max(10));
    (0..count).map(|_| PooledTransactionVariant::try_from(tx_gen.gen_eip1559()).unwrap()).collect()
}

fn recover_current(txs: Vec<PooledTransactionVariant>) -> Vec<EthPooledTransaction> {
    txs.into_par_iter()
        .filter_map(|tx| {
            SignedTransaction::try_into_recovered(tx).ok().map(EthPooledTransaction::from_pooled)
        })
        .collect()
}

fn recover_sequential(txs: Vec<PooledTransactionVariant>) -> Vec<EthPooledTransaction> {
    txs.into_iter()
        .filter_map(|tx| {
            SignedTransaction::try_into_recovered(tx).ok().map(EthPooledTransaction::from_pooled)
        })
        .collect()
}

fn recover_signed_parallel_with_min_len(
    txs: Vec<PooledTransactionVariant>,
    min_len: usize,
) -> Vec<EthPooledTransaction> {
    txs.into_par_iter()
        .with_min_len(min_len)
        .filter_map(|tx| {
            SignedTransaction::try_into_recovered(tx).ok().map(EthPooledTransaction::from_pooled)
        })
        .collect()
}

fn recover_sequential_with_buf(txs: Vec<PooledTransactionVariant>) -> Vec<EthPooledTransaction> {
    let mut buf = Vec::new();
    txs.into_iter()
        .filter_map(|tx| {
            tx.recover_with_buf(&mut buf)
                .ok()
                .map(|signer| EthPooledTransaction::from_pooled(tx.with_signer(signer)))
        })
        .collect()
}

fn recover_parallel_with_min_len(
    txs: Vec<PooledTransactionVariant>,
    min_len: usize,
) -> Vec<EthPooledTransaction> {
    txs.into_par_iter()
        .with_min_len(min_len)
        .map_init(Vec::new, |buf, tx| {
            tx.recover_with_buf(buf)
                .ok()
                .map(|signer| EthPooledTransaction::from_pooled(tx.with_signer(signer)))
        })
        .filter_map(|tx| tx)
        .collect()
}

fn recover_hybrid_with_buf(txs: Vec<PooledTransactionVariant>) -> Vec<EthPooledTransaction> {
    if txs.len() < DEFAULT_MIN_PARALLEL_BATCH {
        recover_sequential_with_buf(txs)
    } else {
        recover_parallel_with_min_len(txs, DEFAULT_MIN_PARALLEL_BATCH)
    }
}

fn bench_recovery(c: &mut Criterion) {
    let mut group = c.benchmark_group("tx_recovery");

    for batch_size in BATCH_SIZES {
        let txs = generate_txs(batch_size);
        group.throughput(Throughput::Elements(batch_size as u64));

        group.bench_with_input(BenchmarkId::new("current/parallel", batch_size), &txs, |b, txs| {
            b.iter_batched(|| txs.clone(), recover_current, BatchSize::SmallInput)
        });

        group.bench_with_input(BenchmarkId::new("sequential", batch_size), &txs, |b, txs| {
            b.iter_batched(|| txs.clone(), recover_sequential, BatchSize::SmallInput)
        });

        group.bench_with_input(BenchmarkId::new("sequential/buf", batch_size), &txs, |b, txs| {
            b.iter_batched(|| txs.clone(), recover_sequential_with_buf, BatchSize::SmallInput)
        });

        group.bench_with_input(
            BenchmarkId::new("hybrid/min-len-16/buf", batch_size),
            &txs,
            |b, txs| b.iter_batched(|| txs.clone(), recover_hybrid_with_buf, BatchSize::SmallInput),
        );

        for min_len in MIN_PARALLEL_SPLIT_LENS {
            group.bench_with_input(
                BenchmarkId::new(format!("parallel/min-len-{min_len}"), batch_size),
                &txs,
                |b, txs| {
                    b.iter_batched(
                        || txs.clone(),
                        |txs| recover_signed_parallel_with_min_len(txs, min_len),
                        BatchSize::SmallInput,
                    )
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("parallel/min-len-{min_len}/buf"), batch_size),
                &txs,
                |b, txs| {
                    b.iter_batched(
                        || txs.clone(),
                        |txs| recover_parallel_with_min_len(txs, min_len),
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
