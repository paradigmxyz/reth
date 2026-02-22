#![allow(missing_docs)]
//! Benchmarks for `StorageBloomFilter`.
//!
//! Measures insert and lookup throughput to validate that the bloom filter
//! adds negligible overhead compared to MDBX seeks (~1-10us).

use alloy_primitives::{address, Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_storage_bloom::StorageBloomFilter;

/// Generate deterministic (address, slot) pairs for benchmarking.
fn generate_pairs(n: u64) -> Vec<(Address, B256)> {
    let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
    (0..n).map(|i| (addr, B256::from(U256::from(i)))).collect()
}

/// Generate absent pairs (different address, so guaranteed not in bloom after inserting
/// pairs from `generate_pairs`).
fn generate_absent_pairs(n: u64) -> Vec<(Address, B256)> {
    let addr = address!("1111111111111111111111111111111111111111");
    (0..n).map(|i| (addr, B256::from(U256::from(i)))).collect()
}

fn bench_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("bloom_insert");

    for size_mb in [16, 128, 256] {
        let bloom = StorageBloomFilter::with_size_mb(size_mb);
        let pairs = generate_pairs(10_000);

        group.bench_with_input(
            BenchmarkId::new("insert", format!("{size_mb}MB")),
            &pairs,
            |b, pairs| {
                b.iter(|| {
                    for (addr, slot) in pairs {
                        bloom.insert(addr, slot);
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_might_contain_true_negative(c: &mut Criterion) {
    let mut group = c.benchmark_group("bloom_true_negative");

    for size_mb in [16, 128, 256] {
        let bloom = StorageBloomFilter::with_size_mb(size_mb);
        let pairs = generate_pairs(100_000);
        for (addr, slot) in &pairs {
            bloom.insert(addr, slot);
        }

        let absent = generate_absent_pairs(10_000);

        group.bench_with_input(
            BenchmarkId::new("might_contain_miss", format!("{size_mb}MB")),
            &absent,
            |b, absent| {
                b.iter(|| {
                    for (addr, slot) in absent {
                        std::hint::black_box(bloom.might_contain(addr, slot));
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_might_contain_true_positive(c: &mut Criterion) {
    let mut group = c.benchmark_group("bloom_true_positive");

    for size_mb in [16, 128, 256] {
        let bloom = StorageBloomFilter::with_size_mb(size_mb);
        let pairs = generate_pairs(100_000);
        for (addr, slot) in &pairs {
            bloom.insert(addr, slot);
        }

        // Query the same pairs we inserted — all should be true positives
        let query: Vec<_> = pairs.iter().take(10_000).copied().collect();

        group.bench_with_input(
            BenchmarkId::new("might_contain_hit", format!("{size_mb}MB")),
            &query,
            |b, query| {
                b.iter(|| {
                    for (addr, slot) in query {
                        std::hint::black_box(bloom.might_contain(addr, slot));
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("bloom_mixed_workload");

    // Simulate realistic workload: 35% misses, 65% hits (based on Nethermind data)
    let bloom = StorageBloomFilter::with_size_mb(256);
    let hit_pairs = generate_pairs(65_000);
    for (addr, slot) in &hit_pairs {
        bloom.insert(addr, slot);
    }

    let miss_pairs = generate_absent_pairs(35_000);

    // Interleave hits and misses
    let mut mixed: Vec<(Address, B256, bool)> = Vec::with_capacity(10_000);
    let mut hit_idx = 0;
    let mut miss_idx = 0;
    for i in 0..10_000u64 {
        if i % 100 < 35 {
            // 35% miss
            mixed.push((miss_pairs[miss_idx].0, miss_pairs[miss_idx].1, false));
            miss_idx += 1;
        } else {
            // 65% hit
            mixed.push((hit_pairs[hit_idx].0, hit_pairs[hit_idx].1, true));
            hit_idx += 1;
        }
    }

    group.bench_function("35pct_miss_65pct_hit", |b| {
        b.iter(|| {
            let mut short_circuited = 0u64;
            for (addr, slot, _expected) in &mixed {
                if !bloom.might_contain(addr, slot) {
                    short_circuited += 1;
                    continue;
                }
                // In production this would be an MDBX seek.
                // Here we just simulate the bloom check overhead.
                std::hint::black_box(());
            }
            std::hint::black_box(short_circuited);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_insert,
    bench_might_contain_true_negative,
    bench_might_contain_true_positive,
    bench_mixed_workload,
);
criterion_main!(benches);
