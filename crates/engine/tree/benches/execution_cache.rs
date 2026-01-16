//! Heavy benchmarks for execution cache performance.
//!
//! Based on #eng-perf discussions about:
//! - moka vs mini-moka cache hit rates
//! - Cache contention under high throughput
//! - Pre-warming effectiveness
//! - 4GB fixed_cache allocation overhead
//!
//! Run with: cargo bench -p reth-engine-tree --bench execution_cache

#![allow(missing_docs)]

use alloy_primitives::{Address, B256, Bytes, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mini_moka::sync::CacheBuilder;
use rand::Rng;
use reth_primitives_traits::{Account, Bytecode};
use revm_primitives::map::DefaultHashBuilder;
use std::{sync::Arc, thread, time::Duration};

type Cache<K, V> = mini_moka::sync::Cache<K, V, DefaultHashBuilder>;

/// Cache configuration matching production settings
struct CacheConfig {
    account_cache_size: u64,
    storage_cache_size: u64,
    code_cache_size: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            account_cache_size: 1_000_000,
            storage_cache_size: 10_000_000,
            code_cache_size: 100_000,
        }
    }
}

fn create_caches(config: &CacheConfig) -> (Cache<B256, Option<Account>>, Cache<(B256, B256), U256>, Cache<B256, Arc<Bytes>>) {
    let account_cache = CacheBuilder::new(config.account_cache_size)
        .time_to_idle(Duration::from_secs(300))
        .build_with_hasher(DefaultHashBuilder::default());

    let storage_cache = CacheBuilder::new(config.storage_cache_size)
        .time_to_idle(Duration::from_secs(300))
        .build_with_hasher(DefaultHashBuilder::default());

    let code_cache = CacheBuilder::new(config.code_cache_size)
        .time_to_idle(Duration::from_secs(300))
        .build_with_hasher(DefaultHashBuilder::default());

    (account_cache, storage_cache, code_cache)
}

/// Benchmark: Cache lookup performance under varying hit rates
fn bench_cache_hit_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache/hit_rates");
    group.sample_size(50);

    let config = CacheConfig::default();
    let hit_rates = [0.45, 0.78, 0.90, 0.95]; // 45% baseline, 78% with Half-Path, optimized

    for hit_rate in hit_rates {
        let id = format!("hit_rate_{:.0}pct", hit_rate * 100.0);
        group.throughput(Throughput::Elements(10000));

        group.bench_function(BenchmarkId::new("storage_lookups", &id), |b| {
            b.iter_with_setup(
                || {
                    let (_, storage_cache, _) = create_caches(&config);
                    let mut rng = rand::rng();

                    // Pre-populate cache with some entries
                    let cached_keys: Vec<(B256, B256)> = (0..10000)
                        .map(|_| (B256::random(), B256::random()))
                        .collect();

                    for (addr, slot) in &cached_keys {
                        storage_cache.insert((*addr, *slot), U256::from(rng.random::<u64>()));
                    }

                    // Create lookup keys - mix of cached and uncached
                    let num_cached = (10000.0 * hit_rate) as usize;
                    let mut lookup_keys: Vec<(B256, B256)> = cached_keys[..num_cached].to_vec();
                    lookup_keys.extend((0..(10000 - num_cached)).map(|_| (B256::random(), B256::random())));

                    // Shuffle
                    use rand::seq::SliceRandom;
                    lookup_keys.shuffle(&mut rng);

                    (storage_cache, lookup_keys)
                },
                |(cache, keys)| {
                    let mut hits = 0u64;
                    let mut misses = 0u64;
                    for (addr, slot) in keys {
                        if cache.get(&(addr, slot)).is_some() {
                            hits += 1;
                        } else {
                            misses += 1;
                        }
                    }
                    (hits, misses)
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: Concurrent cache access (simulating rayon parallel execution)
fn bench_cache_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache/contention");
    group.sample_size(20);

    let thread_counts = [1, 4, 8, 16, 32]; // Match rayon thread pool sizes

    for num_threads in thread_counts {
        let id = format!("threads_{}", num_threads);
        group.throughput(Throughput::Elements(100000));

        group.bench_function(BenchmarkId::new("concurrent_storage_access", &id), |b| {
            b.iter_with_setup(
                || {
                    let config = CacheConfig::default();
                    let (_, storage_cache, _) = create_caches(&config);
                    let storage_cache = Arc::new(storage_cache);

                    // Pre-populate with hot data
                    let mut rng = rand::rng();
                    for _ in 0..50000 {
                        let key = (B256::random(), B256::random());
                        storage_cache.insert(key, U256::from(rng.random::<u64>()));
                    }

                    storage_cache
                },
                |cache| {
                    let handles: Vec<_> = (0..num_threads)
                        .map(|_| {
                            let cache = Arc::clone(&cache);
                            thread::spawn(move || {
                                let mut rng = rand::rng();
                                let ops_per_thread = 100000 / num_threads;
                                let mut hits = 0u64;

                                for _ in 0..ops_per_thread {
                                    let key = (B256::random(), B256::random());
                                    // 70% reads, 30% writes
                                    if rng.random_bool(0.7) {
                                        if cache.get(&key).is_some() {
                                            hits += 1;
                                        }
                                    } else {
                                        cache.insert(key, U256::from(rng.random::<u64>()));
                                    }
                                }
                                hits
                            })
                        })
                        .collect();

                    let total_hits: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
                    total_hits
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: Cache insertion burst (simulating BundleState merge after block)
fn bench_cache_burst_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache/burst_insert");
    group.sample_size(20);

    // Sizes from Slack discussions:
    // - Normal block: ~2000 storage changes
    // - Heavy block: ~10000 storage changes
    // - Megablock: ~50000 storage changes
    let burst_sizes = [2000, 10000, 50000];

    for size in burst_sizes {
        let id = format!("entries_{}", size);
        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(BenchmarkId::new("storage_burst", &id), |b| {
            b.iter_with_setup(
                || {
                    let config = CacheConfig::default();
                    let (_, storage_cache, _) = create_caches(&config);

                    let mut rng = rand::rng();
                    let entries: Vec<((B256, B256), U256)> = (0..size)
                        .map(|_| {
                            ((B256::random(), B256::random()), U256::from(rng.random::<u64>()))
                        })
                        .collect();

                    (storage_cache, entries)
                },
                |(cache, entries)| {
                    for (key, value) in entries {
                        cache.insert(key, value);
                    }
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: Simulating TIP-20 token transfer patterns (Tempo-specific)
/// These trigger mostly cache misses per Slack discussion
fn bench_tip20_cache_pattern(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache/tip20_pattern");
    group.sample_size(20);

    // TIP-20 transfers access unique accounts rarely seen before
    let transfer_counts = [100, 500, 1000];

    for num_transfers in transfer_counts {
        let id = format!("transfers_{}", num_transfers);
        group.throughput(Throughput::Elements(num_transfers as u64));

        group.bench_function(BenchmarkId::new("unique_account_access", &id), |b| {
            b.iter_with_setup(
                || {
                    let config = CacheConfig::default();
                    let (account_cache, storage_cache, _) = create_caches(&config);

                    // TIP-20: Each transfer accesses sender, recipient, fee token contract
                    // Most are unique addresses → cache misses
                    let mut rng = rand::rng();
                    let transfer_accounts: Vec<(B256, B256, B256)> = (0..num_transfers)
                        .map(|_| (B256::random(), B256::random(), B256::random()))
                        .collect();

                    (account_cache, storage_cache, transfer_accounts)
                },
                |(account_cache, storage_cache, transfers)| {
                    let mut account_misses = 0u64;
                    let mut storage_misses = 0u64;

                    for (sender, recipient, fee_contract) in transfers {
                        // Check sender account
                        if account_cache.get(&sender).is_none() {
                            account_misses += 1;
                            // Simulate DB lookup and cache population
                            account_cache.insert(sender, Some(Account::default()));
                        }

                        // Check recipient
                        if account_cache.get(&recipient).is_none() {
                            account_misses += 1;
                            account_cache.insert(recipient, Some(Account::default()));
                        }

                        // Check fee contract storage (balance slot)
                        let balance_slot = B256::ZERO;
                        if storage_cache.get(&(fee_contract, balance_slot)).is_none() {
                            storage_misses += 1;
                            storage_cache.insert((fee_contract, balance_slot), U256::from(1000));
                        }
                    }

                    (account_misses, storage_misses)
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: Pre-warming effectiveness
fn bench_prewarm_effectiveness(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache/prewarm");
    group.sample_size(20);

    let block_sizes = [500, 2000, 5000];

    for num_accounts in block_sizes {
        let id = format!("accounts_{}", num_accounts);

        // Without pre-warming
        group.bench_function(BenchmarkId::new("cold_execution", &id), |b| {
            b.iter_with_setup(
                || {
                    let config = CacheConfig::default();
                    let (account_cache, storage_cache, _) = create_caches(&config);

                    let mut rng = rand::rng();
                    let accounts: Vec<B256> = (0..num_accounts).map(|_| B256::random()).collect();

                    (account_cache, storage_cache, accounts)
                },
                |(account_cache, storage_cache, accounts)| {
                    let mut misses = 0u64;
                    for addr in &accounts {
                        if account_cache.get(addr).is_none() {
                            misses += 1;
                            // Simulate expensive DB lookup
                            std::hint::black_box(0u64);
                            account_cache.insert(*addr, Some(Account::default()));
                        }
                        // Access storage
                        let slot = B256::ZERO;
                        if storage_cache.get(&(*addr, slot)).is_none() {
                            std::hint::black_box(0u64);
                            storage_cache.insert((*addr, slot), U256::ZERO);
                        }
                    }
                    misses
                },
            );
        });

        // With pre-warming
        group.bench_function(BenchmarkId::new("warm_execution", &id), |b| {
            b.iter_with_setup(
                || {
                    let config = CacheConfig::default();
                    let (account_cache, storage_cache, _) = create_caches(&config);

                    let mut rng = rand::rng();
                    let accounts: Vec<B256> = (0..num_accounts).map(|_| B256::random()).collect();

                    // Pre-warm the cache
                    for addr in &accounts {
                        account_cache.insert(*addr, Some(Account::default()));
                        storage_cache.insert((*addr, B256::ZERO), U256::ZERO);
                    }

                    (account_cache, storage_cache, accounts)
                },
                |(account_cache, storage_cache, accounts)| {
                    let mut hits = 0u64;
                    for addr in &accounts {
                        if account_cache.get(addr).is_some() {
                            hits += 1;
                        }
                        let slot = B256::ZERO;
                        if storage_cache.get(&(*addr, slot)).is_some() {
                            hits += 1;
                        }
                    }
                    hits
                },
            );
        });
    }

    group.finish();
}

criterion_group!(
    name = execution_cache;
    config = Criterion::default().significance_level(0.05).sample_size(20);
    targets =
        bench_cache_hit_rates,
        bench_cache_contention,
        bench_cache_burst_insert,
        bench_tip20_cache_pattern,
        bench_prewarm_effectiveness
);
criterion_main!(execution_cache);
