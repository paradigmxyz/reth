//! Benchmarks for `TxEnv` allocation patterns.
//!
//! Measures the performance difference between:
//! 1. Clone path: `to_tx_env()` which clones the `TxEnv` data
//! 2. Move path: `into_tx_env()` which moves ownership without cloning
//!
//! The key insight is that `WithTxEnv` already contains a prepared `TxEnv`,
//! so extracting it via `into_tx_env()` avoids the clone that `to_tx_env()` performs.

use alloy_consensus::{TxEip1559, TxLegacy};
use alloy_eips::eip2930::{AccessList, AccessListItem};
use alloy_evm::ToTxEnv;
use alloy_primitives::{address, Address, Bytes, TxKind, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_evm::{execute::WithTxEnv, tx_env::FillTxEnv};
use reth_primitives_traits::Recovered;
use revm::context::TxEnv;
use std::sync::Arc;

const SENDER: Address = address!("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd");
const RECIPIENT: Address = address!("0x1234567890123456789012345678901234567890");

fn create_legacy_txs(count: usize) -> Vec<Recovered<TxLegacy>> {
    (0..count)
        .map(|i| {
            let tx = TxLegacy {
                chain_id: Some(1),
                nonce: i as u64,
                gas_price: 100 + i as u128,
                gas_limit: 21000,
                to: TxKind::Call(RECIPIENT),
                value: U256::from(1000 + i),
                input: Bytes::from(vec![0u8; 100]),
            };
            Recovered::new_unchecked(tx, SENDER)
        })
        .collect()
}

fn create_eip1559_txs_with_access_list(
    count: usize,
    access_list_size: usize,
) -> Vec<Recovered<TxEip1559>> {
    let access_list: Vec<AccessListItem> = (0..access_list_size)
        .map(|i| AccessListItem {
            address: Address::with_last_byte(i as u8),
            storage_keys: vec![B256::with_last_byte(i as u8); 3],
        })
        .collect();

    (0..count)
        .map(|i| {
            let tx = TxEip1559 {
                chain_id: 1,
                nonce: i as u64,
                gas_limit: 100000,
                max_fee_per_gas: 100,
                max_priority_fee_per_gas: 10,
                to: TxKind::Call(RECIPIENT),
                value: U256::from(1000),
                input: Bytes::from(vec![0u8; 200]),
                access_list: AccessList(access_list.clone()),
            };
            Recovered::new_unchecked(tx, SENDER)
        })
        .collect()
}

/// Prepares `WithTxEnv` wrappers
fn prepare_with_tx_envs<T: Clone>(txs: &[Recovered<T>]) -> Vec<WithTxEnv<TxEnv, Recovered<T>>>
where
    TxEnv: FillTxEnv<T>,
{
    txs.iter()
        .map(|recovered| {
            let mut tx_env = TxEnv::default();
            tx_env.fill_from_recovered_tx(recovered.inner(), recovered.signer());
            WithTxEnv { tx_env, tx: Arc::new(recovered.clone()) }
        })
        .collect()
}

/// Benchmark comparing the conversion costs.
///
/// - `to_tx_env_clone`: Calls `to_tx_env()` on `WithTxEnv` which clones the inner `TxEnv`
/// - `into_tx_env_move`: Calls `into_tx_env()` on `WithTxEnv` which moves the inner `TxEnv`
fn bench_tx_env_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("tx_env_extraction");

    for tx_count in [100, 500, 1000] {
        group.throughput(Throughput::Elements(tx_count as u64));

        let txs = create_legacy_txs(tx_count);
        let with_tx_envs = prepare_with_tx_envs(&txs);

        // Benchmark: Clone via to_tx_env (reference path)
        group.bench_with_input(
            BenchmarkId::new("to_tx_env_clone", tx_count),
            &with_tx_envs,
            |b, with_tx_envs| {
                b.iter(|| {
                    for with_tx_env in with_tx_envs {
                        // This clones the TxEnv
                        let tx_env: TxEnv = with_tx_env.to_tx_env();
                        black_box(tx_env);
                    }
                });
            },
        );

        // Benchmark: Move via into_tx_env (zero-copy path)
        // We use iter_batched because into_tx_env consumes self
        group.bench_with_input(
            BenchmarkId::new("into_tx_env_move", tx_count),
            &with_tx_envs,
            |b, with_tx_envs| {
                b.iter_batched(
                    || with_tx_envs.clone(),
                    |owned| {
                        for with_tx_env in owned {
                            // This moves the TxEnv without cloning
                            let tx_env: TxEnv = with_tx_env.into_tx_env();
                            black_box(tx_env);
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmark EIP-1559 transactions with varying access list sizes
fn bench_tx_env_extraction_eip1559(c: &mut Criterion) {
    let mut group = c.benchmark_group("tx_env_extraction_eip1559");

    // More access list entries = more data to clone = bigger difference
    for access_list_size in [0, 10, 50, 100] {
        let tx_count = 100;
        let label = format!("acl_size={}", access_list_size);
        group.throughput(Throughput::Elements(tx_count as u64));

        let txs = create_eip1559_txs_with_access_list(tx_count, access_list_size);
        let with_tx_envs = prepare_with_tx_envs(&txs);

        group.bench_with_input(
            BenchmarkId::new("to_tx_env_clone", &label),
            &with_tx_envs,
            |b, with_tx_envs| {
                b.iter(|| {
                    for with_tx_env in with_tx_envs {
                        let tx_env: TxEnv = with_tx_env.to_tx_env();
                        black_box(tx_env);
                    }
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("into_tx_env_move", &label),
            &with_tx_envs,
            |b, with_tx_envs| {
                b.iter_batched(
                    || with_tx_envs.clone(),
                    |owned| {
                        for with_tx_env in owned {
                            let tx_env: TxEnv = with_tx_env.into_tx_env();
                            black_box(tx_env);
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Benchmarks the `TxEnv` reuse pattern for consecutive transactions
fn bench_tx_env_reuse(c: &mut Criterion) {
    use reth_evm::tx_env::ReusableTxEnv;

    let mut group = c.benchmark_group("tx_env_reuse");

    for tx_count in [100, 500, 1000] {
        group.throughput(Throughput::Elements(tx_count as u64));

        let txs = create_legacy_txs(tx_count);

        group.bench_with_input(BenchmarkId::new("new_each_time", tx_count), &txs, |b, txs| {
            b.iter(|| {
                for tx in txs {
                    let mut tx_env = TxEnv::default();
                    tx_env.fill_from_recovered_tx(tx.inner(), tx.signer());
                    black_box(&tx_env);
                }
            });
        });

        group.bench_with_input(BenchmarkId::new("reusable_clear", tx_count), &txs, |b, txs| {
            b.iter(|| {
                let mut reusable = ReusableTxEnv::new();
                for tx in txs {
                    reusable.fill(tx.inner(), tx.signer());
                    black_box(reusable.as_tx_env());
                }
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_tx_env_extraction,
    bench_tx_env_extraction_eip1559,
    bench_tx_env_reuse
);
criterion_main!(benches);
