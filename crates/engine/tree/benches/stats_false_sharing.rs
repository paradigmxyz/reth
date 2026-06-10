#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_engine_tree::tree::instrumented_state::StateProviderStats;
use reth_execution_cache::CacheStats;
use std::{
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier,
    },
    thread,
    time::{Duration, Instant},
};

const ITERATIONS_PER_THREAD: usize = 50_000;
const MAX_THREADS: usize = 8;
const NANOS_PER_SEC: u64 = 1_000_000_000;

#[derive(Default)]
struct UnpaddedAtomicDuration {
    nanos: AtomicU64,
}

impl UnpaddedAtomicDuration {
    fn duration(&self) -> Duration {
        let nanos = self.nanos.load(Ordering::Relaxed);
        Duration::new(nanos / NANOS_PER_SEC, (nanos % NANOS_PER_SEC) as u32)
    }

    fn add_duration(&self, duration: Duration) {
        let nanos = duration.as_secs() * NANOS_PER_SEC + duration.subsec_nanos() as u64;
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct UnpaddedStateProviderStats {
    total_storage_fetches: AtomicUsize,
    total_storage_fetch_latency: UnpaddedAtomicDuration,

    total_code_fetches: AtomicUsize,
    total_code_fetch_latency: UnpaddedAtomicDuration,
    total_code_fetched_bytes: AtomicUsize,

    total_account_fetches: AtomicUsize,
    total_account_fetch_latency: UnpaddedAtomicDuration,
}

impl UnpaddedStateProviderStats {
    fn record_storage_fetch(&self, latency: Duration) {
        self.total_storage_fetches.fetch_add(1, Ordering::Relaxed);
        self.total_storage_fetch_latency.add_duration(latency);
    }

    fn record_code_fetch(&self, latency: Duration, bytes: usize) {
        self.total_code_fetches.fetch_add(1, Ordering::Relaxed);
        self.total_code_fetch_latency.add_duration(latency);
        self.total_code_fetched_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn record_account_fetch(&self, latency: Duration) {
        self.total_account_fetches.fetch_add(1, Ordering::Relaxed);
        self.total_account_fetch_latency.add_duration(latency);
    }

    fn checksum(&self) -> usize {
        self.total_storage_fetches.load(Ordering::Relaxed) +
            self.total_storage_fetch_latency.duration().as_nanos() as usize +
            self.total_code_fetches.load(Ordering::Relaxed) +
            self.total_code_fetch_latency.duration().as_nanos() as usize +
            self.total_code_fetched_bytes.load(Ordering::Relaxed) +
            self.total_account_fetches.load(Ordering::Relaxed) +
            self.total_account_fetch_latency.duration().as_nanos() as usize
    }
}

#[derive(Default)]
struct UnpaddedCacheStats {
    account_hits: AtomicUsize,
    account_misses: AtomicUsize,
    storage_hits: AtomicUsize,
    storage_misses: AtomicUsize,
    code_hits: AtomicUsize,
    code_misses: AtomicUsize,
}

impl UnpaddedCacheStats {
    fn record_account_hit(&self) {
        self.account_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_account_miss(&self) {
        self.account_misses.fetch_add(1, Ordering::Relaxed);
    }

    fn record_storage_hit(&self) {
        self.storage_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_storage_miss(&self) {
        self.storage_misses.fetch_add(1, Ordering::Relaxed);
    }

    fn record_code_hit(&self) {
        self.code_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_code_miss(&self) {
        self.code_misses.fetch_add(1, Ordering::Relaxed);
    }

    fn checksum(&self) -> usize {
        self.account_hits.load(Ordering::Relaxed) +
            self.account_misses.load(Ordering::Relaxed) +
            self.storage_hits.load(Ordering::Relaxed) +
            self.storage_misses.load(Ordering::Relaxed) +
            self.code_hits.load(Ordering::Relaxed) +
            self.code_misses.load(Ordering::Relaxed)
    }
}

fn worker_count() -> usize {
    thread::available_parallelism().map_or(1, |parallelism| parallelism.get()).clamp(1, MAX_THREADS)
}

fn measure_unpadded_state_stats(workers: usize, repetitions: u64) -> Duration {
    let stats = Arc::new(UnpaddedStateProviderStats::default());
    let barrier = Arc::new(Barrier::new(workers + 1));
    let latency = Duration::from_nanos(37);
    let handles = (0..workers)
        .map(|worker| {
            let stats = Arc::clone(&stats);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..repetitions {
                    for _ in 0..ITERATIONS_PER_THREAD {
                        match worker % 3 {
                            0 => stats.record_storage_fetch(latency),
                            1 => stats.record_code_fetch(latency, 32),
                            _ => stats.record_account_fetch(latency),
                        }
                    }
                }
                barrier.wait();
            })
        })
        .collect::<Vec<_>>();

    let start = Instant::now();
    barrier.wait();
    barrier.wait();
    let elapsed = start.elapsed();

    for handle in handles {
        handle.join().unwrap();
    }

    black_box(stats.checksum());
    elapsed
}

fn measure_padded_state_stats(workers: usize, repetitions: u64) -> Duration {
    let stats = Arc::new(StateProviderStats::default());
    let barrier = Arc::new(Barrier::new(workers + 1));
    let latency = Duration::from_nanos(37);
    let handles = (0..workers)
        .map(|worker| {
            let stats = Arc::clone(&stats);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..repetitions {
                    for _ in 0..ITERATIONS_PER_THREAD {
                        match worker % 3 {
                            0 => stats.record_storage_fetch(latency),
                            1 => stats.record_code_fetch(latency, 32),
                            _ => stats.record_account_fetch(latency),
                        }
                    }
                }
                barrier.wait();
            })
        })
        .collect::<Vec<_>>();

    let start = Instant::now();
    barrier.wait();
    barrier.wait();
    let elapsed = start.elapsed();

    for handle in handles {
        handle.join().unwrap();
    }

    let checksum = stats.total_storage_fetches() +
        stats.total_storage_fetch_latency().as_nanos() as usize +
        stats.total_code_fetches() +
        stats.total_code_fetch_latency().as_nanos() as usize +
        stats.total_code_fetched_bytes() +
        stats.total_account_fetches() +
        stats.total_account_fetch_latency().as_nanos() as usize;
    black_box(checksum);
    elapsed
}

fn measure_unpadded_cache_stats(workers: usize, repetitions: u64) -> Duration {
    let stats = Arc::new(UnpaddedCacheStats::default());
    let barrier = Arc::new(Barrier::new(workers + 1));
    let handles = (0..workers)
        .map(|worker| {
            let stats = Arc::clone(&stats);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..repetitions {
                    for _ in 0..ITERATIONS_PER_THREAD {
                        match worker % 6 {
                            0 => stats.record_account_hit(),
                            1 => stats.record_account_miss(),
                            2 => stats.record_storage_hit(),
                            3 => stats.record_storage_miss(),
                            4 => stats.record_code_hit(),
                            _ => stats.record_code_miss(),
                        }
                    }
                }
                barrier.wait();
            })
        })
        .collect::<Vec<_>>();

    let start = Instant::now();
    barrier.wait();
    barrier.wait();
    let elapsed = start.elapsed();

    for handle in handles {
        handle.join().unwrap();
    }

    black_box(stats.checksum());
    elapsed
}

fn measure_padded_cache_stats(workers: usize, repetitions: u64) -> Duration {
    let stats = Arc::new(CacheStats::default());
    let barrier = Arc::new(Barrier::new(workers + 1));
    let handles = (0..workers)
        .map(|worker| {
            let stats = Arc::clone(&stats);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..repetitions {
                    for _ in 0..ITERATIONS_PER_THREAD {
                        match worker % 6 {
                            0 => stats.record_account_hit(),
                            1 => stats.record_account_miss(),
                            2 => stats.record_storage_hit(),
                            3 => stats.record_storage_miss(),
                            4 => stats.record_code_hit(),
                            _ => stats.record_code_miss(),
                        }
                    }
                }
                barrier.wait();
            })
        })
        .collect::<Vec<_>>();

    let start = Instant::now();
    barrier.wait();
    barrier.wait();
    let elapsed = start.elapsed();

    for handle in handles {
        handle.join().unwrap();
    }

    let checksum = stats.account_hits() +
        stats.account_misses() +
        stats.storage_hits() +
        stats.storage_misses() +
        stats.code_hits() +
        stats.code_misses();
    black_box(checksum);
    elapsed
}

fn stats_false_sharing(c: &mut Criterion) {
    let workers = worker_count();
    let state_workers = workers.min(3);
    let cache_workers = workers.min(6);

    let mut group = c.benchmark_group("stats_false_sharing/state_provider");
    group.bench_with_input(
        BenchmarkId::new("split_fetch_fields", "unpadded"),
        &state_workers,
        |b, &workers| {
            b.iter_custom(|repetitions| measure_unpadded_state_stats(workers, repetitions));
        },
    );
    group.bench_with_input(
        BenchmarkId::new("split_fetch_fields", "cache_padded"),
        &state_workers,
        |b, &workers| {
            b.iter_custom(|repetitions| measure_padded_state_stats(workers, repetitions));
        },
    );
    group.finish();

    let mut group = c.benchmark_group("stats_false_sharing/cache_stats");
    group.bench_with_input(
        BenchmarkId::new("split_hit_fields", "unpadded"),
        &cache_workers,
        |b, &workers| {
            b.iter_custom(|repetitions| measure_unpadded_cache_stats(workers, repetitions));
        },
    );
    group.bench_with_input(
        BenchmarkId::new("split_hit_fields", "cache_padded"),
        &cache_workers,
        |b, &workers| {
            b.iter_custom(|repetitions| measure_padded_cache_stats(workers, repetitions));
        },
    );
    group.finish();
}

criterion_group!(benches, stats_false_sharing);
criterion_main!(benches);
