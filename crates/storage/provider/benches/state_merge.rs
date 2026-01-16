//! Benchmarks for state merge strategies.
//!
//! Compares:
//! - Flatten + Sort: Collect all items into one Vec, sort globally
//! - K-way Merge: Pre-sort each block, then k-way merge sorted slices

#![allow(missing_docs, deprecated)]

use alloy_primitives::{Address, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rayon::slice::ParallelSliceMut;
use revm_database::states::{PlainStorageChangeset, StateChangeset};
use revm_state::AccountInfo;
use std::cmp::Ordering;

/// Generate random address
fn random_address(rng: &mut StdRng) -> Address {
    Address::from_slice(&rng.random::<[u8; 20]>())
}

/// Generate random B256
#[allow(dead_code)]
fn random_b256(rng: &mut StdRng) -> B256 {
    B256::from_slice(&rng.random::<[u8; 32]>())
}

/// Generate test StateChangesets simulating multiple blocks
fn generate_changesets(
    num_blocks: usize,
    accounts_per_block: usize,
    storage_per_account: usize,
    seed: u64,
) -> Vec<StateChangeset> {
    let mut rng = StdRng::seed_from_u64(seed);

    (0..num_blocks)
        .map(|_| {
            let accounts: Vec<_> = (0..accounts_per_block)
                .map(|_| {
                    let addr = random_address(&mut rng);
                    let info = if rng.random_bool(0.1) {
                        None // 10% destroyed
                    } else {
                        Some(AccountInfo {
                            balance: U256::from(rng.random::<u64>()),
                            nonce: rng.random(),
                            code_hash: B256::ZERO,
                            code: None,
                        })
                    };
                    (addr, info)
                })
                .collect();

            let storage: Vec<_> = (0..accounts_per_block / 3)
                .map(|_| {
                    let addr = random_address(&mut rng);
                    let slots: Vec<_> = (0..storage_per_account)
                        .map(|_| {
                            (
                                U256::from_be_bytes(rng.random::<[u8; 32]>()),
                                U256::from(rng.random::<u64>()),
                            )
                        })
                        .collect();
                    PlainStorageChangeset { address: addr, wipe_storage: false, storage: slots }
                })
                .collect();

            StateChangeset { accounts, storage, contracts: vec![] }
        })
        .collect()
}

/// Sequential sort approach
fn flatten_and_sort_accounts(changesets: &[StateChangeset]) -> Vec<(Address, usize, bool)> {
    let mut all: Vec<(Address, usize, bool)> = Vec::new();
    for (block_idx, cs) in changesets.iter().enumerate() {
        for (addr, info) in &cs.accounts {
            all.push((*addr, block_idx, info.is_some()));
        }
    }
    all.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    all
}

/// Parallel sort approach (optimized)
fn flatten_and_par_sort_accounts(changesets: &[StateChangeset]) -> Vec<(Address, usize, bool)> {
    let mut all: Vec<(Address, usize, bool)> = Vec::new();
    for (block_idx, cs) in changesets.iter().enumerate() {
        for (addr, info) in &cs.accounts {
            all.push((*addr, block_idx, info.is_some()));
        }
    }
    all.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    all
}

fn bench_account_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("account_merge");

    // Test with varying number of blocks and accounts per block
    // Realistic mainnet: ~100-500 accounts touched per block, batches of 10-100 blocks
    for (num_blocks, accounts_per_block) in [
        (10, 500),   // Small batch, moderate activity
        (50, 500),   // Medium batch, moderate activity
        (100, 500),  // Large batch, moderate activity
        (50, 1000),  // Medium batch, heavy activity
        (100, 1000), // Large batch, heavy activity (stress test)
    ] {
        let total = num_blocks * accounts_per_block;
        group.throughput(Throughput::Elements(total as u64));

        let id = format!("{}blocks_{}acc", num_blocks, accounts_per_block);

        group.bench_with_input(
            BenchmarkId::new("flatten_sort", &id),
            &(num_blocks, accounts_per_block),
            |b, &(nb, apb)| {
                b.iter_batched(
                    || generate_changesets(nb, apb, 5, 42),
                    |cs| black_box(flatten_and_sort_accounts(&cs)),
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("par_sort", &id),
            &(num_blocks, accounts_per_block),
            |b, &(nb, apb)| {
                b.iter_batched(
                    || generate_changesets(nb, apb, 5, 42),
                    |cs| black_box(flatten_and_par_sort_accounts(&cs)),
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

/// Flatten + Sort for storage (U256 slot keys match revm's StateChangeset)
fn flatten_and_sort_storage(changesets: &[StateChangeset]) -> Vec<(Address, U256, usize, U256)> {
    let mut all: Vec<(Address, U256, usize, U256)> = Vec::new();
    for (block_idx, cs) in changesets.iter().enumerate() {
        for psc in &cs.storage {
            for (slot, value) in &psc.storage {
                all.push((psc.address, *slot, block_idx, *value));
            }
        }
    }
    all.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)).then_with(|| b.2.cmp(&a.2))
    });
    all
}

/// Parallel sort for storage
fn flatten_and_par_sort_storage(
    changesets: &[StateChangeset],
) -> Vec<(Address, U256, usize, U256)> {
    let mut all: Vec<(Address, U256, usize, U256)> = Vec::new();
    for (block_idx, cs) in changesets.iter().enumerate() {
        for psc in &cs.storage {
            for (slot, value) in &psc.storage {
                all.push((psc.address, *slot, block_idx, *value));
            }
        }
    }
    all.par_sort_unstable_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)).then_with(|| b.2.cmp(&a.2))
    });
    all
}

fn bench_storage_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_merge");

    // Realistic mainnet storage: contracts with many slots (DEXes, lending protocols)
    // Each block touches ~50-200 contracts, each with 5-50 slot updates
    for (num_blocks, accounts_per_block, slots_per_account) in [
        (10, 150, 20),  // Small batch: 10 blocks × 50 contracts × 20 slots = 10k slots
        (50, 150, 20),  // Medium batch: 50 blocks × 50 contracts × 20 slots = 50k slots
        (100, 150, 20), // Large batch: 100 blocks × 50 contracts × 20 slots = 100k slots
        (50, 300, 30),  // Medium batch, heavy: 50 blocks × 100 contracts × 30 slots = 150k slots
        (100, 300, 30), // Large batch, heavy: 100 blocks × 100 contracts × 30 slots = 300k slots
    ] {
        let total = num_blocks * (accounts_per_block / 3) * slots_per_account;
        group.throughput(Throughput::Elements(total as u64));

        let id = format!("{}blocks_{}slots", num_blocks, total);

        group.bench_with_input(
            BenchmarkId::new("flatten_sort", &id),
            &(num_blocks, accounts_per_block, slots_per_account),
            |b, &(nb, apb, spa)| {
                b.iter_batched(
                    || generate_changesets(nb, apb, spa, 42),
                    |cs| black_box(flatten_and_sort_storage(&cs)),
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("par_sort", &id),
            &(num_blocks, accounts_per_block, slots_per_account),
            |b, &(nb, apb, spa)| {
                b.iter_batched(
                    || generate_changesets(nb, apb, spa, 42),
                    |cs| black_box(flatten_and_par_sort_storage(&cs)),
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_account_merge, bench_storage_merge);
criterion_main!(benches);
