#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use rand::{rngs::StdRng, Rng, SeedableRng};
use rayon::prelude::*;

/// Synthetic storage data: a list of accounts, each with a list of slots.
type MockStorage = Vec<(Address, Vec<(B256, U256)>)>;

#[derive(Clone, Copy, Debug)]
enum Distribution {
    Uniform(usize),
    Skewed { light: usize, heavy: usize },
}

/// Generate random synthetic data with a fixed seed for reproducibility.
fn generate_data(num_accounts: usize, dist: Distribution) -> MockStorage {
    let mut rng = StdRng::seed_from_u64(42);

    (0..num_accounts)
        .map(|_| {
            let address = Address::random();
            let slot_count = match dist {
                Distribution::Uniform(n) => n,
                Distribution::Skewed { light, heavy } => {
                    // 5% chance of being a heavy account
                    if rng.random_bool(0.05) {
                        heavy
                    } else {
                        light
                    }
                }
            };

            let slots = (0..slot_count)
                .map(|_| (B256::random(), U256::from(rng.random::<u64>())))
                .collect();
            (address, slots)
        })
        .collect()
}

/// Loop sequentially, sort slots sequentially.
fn process_sequential(mut data: MockStorage) {
    for (_addr, slots) in &mut data {
        slots.sort_unstable_by_key(|(k, _)| *k);
    }
}

/// Accounts in parallel, sort slots sequentially.
fn process_par_iter_accounts(mut data: MockStorage) {
    data.par_iter_mut().for_each(|(_addr, slots)| {
        slots.sort_unstable_by_key(|(k, _)| *k);
    });
}

/// Accounts sequentially, sort slots parallel.
fn process_par_sort_slots(mut data: MockStorage) {
    for (_addr, slots) in &mut data {
        slots.par_sort_unstable_by_key(|(k, _)| *k);
    }
}

fn bench_storage_sort(c: &mut Criterion) {
    let mut group = c.benchmark_group("sorting_par_exp");

    let scenarios = vec![
        // Finding Account Threshold.
        ("Acc_Low", 100, Distribution::Uniform(4)),
        ("Acc_Med", 1_000, Distribution::Uniform(4)),
        ("Acc_Med_High", 2_500, Distribution::Uniform(4)),
        ("Acc_High", 10_000, Distribution::Uniform(4)),
        // Finding Slot Threshold.
        ("Slots_Med", 10, Distribution::Uniform(100)),
        ("Slots_High", 10, Distribution::Uniform(5_000)),
        ("Slots_Massive", 10, Distribution::Uniform(50_000)),
        // 10k accounts. Most have 4 slots, 5% have 2k slots.
        ("Skewed_2.5k", 2_500, Distribution::Skewed { light: 4, heavy: 2_000 }),
        ("Skewed_10k", 10_000, Distribution::Skewed { light: 4, heavy: 2_000 }),
    ];

    for (name, accounts, dist) in scenarios {
        let input = generate_data(accounts, dist);

        let total_slots: usize = input.iter().map(|(_, s)| s.len()).sum();
        group.throughput(Throughput::Elements(total_slots as u64));

        // Sequential
        group.bench_with_input(BenchmarkId::new("Sequential", name), &input, |b, data| {
            b.iter_batched(|| data.clone(), process_sequential, BatchSize::LargeInput)
        });

        // Parallel Accounts
        group.bench_with_input(BenchmarkId::new("Par_Accounts", name), &input, |b, data| {
            b.iter_batched(|| data.clone(), process_par_iter_accounts, BatchSize::LargeInput)
        });

        // Parallel Slots
        if let Distribution::Uniform(s) = dist &&
            s >= 100
        {
            group.bench_with_input(BenchmarkId::new("Par_Inner_Slots", name), &input, |b, data| {
                b.iter_batched(|| data.clone(), process_par_sort_slots, BatchSize::LargeInput)
            });
        }
    }

    group.finish();
}

criterion_group!(sorting_par_exp, bench_storage_sort);
criterion_main!(sorting_par_exp);
