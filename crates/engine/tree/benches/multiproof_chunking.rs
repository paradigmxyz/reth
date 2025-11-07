#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_engine_tree::tree::{select_dynamic_chunk_size, WorkerSnapshot};
use std::cmp::min;

const DEFAULT_BASE_CHUNK: usize = 10;
const PER_CHUNK_OVERHEAD_NS: f64 = 50_000.0;
const PER_TARGET_COST_NS: f64 = 3_000.0;
const PER_CHUNK_SPIN: usize = 200_000;
const WORK_UNITS: usize = 512 * 4;

fn simulate_processing(work_units: usize, chunk_size: usize, workers: usize) -> u64 {
    if workers == 0 {
        return 0;
    }
    let mut worker_times = vec![0f64; workers];
    let mut spin = 0u64;
    let mut remaining = work_units;
    while remaining > 0 {
        let current = min(remaining, chunk_size) as f64;
        remaining -= current as usize;
        let duration = PER_CHUNK_OVERHEAD_NS + current * PER_TARGET_COST_NS;
        for _ in 0..PER_CHUNK_SPIN {
            spin = spin.wrapping_add(1);
        }
        let mut idx = 0usize;
        let mut min_time = worker_times[0];
        for (i, time) in worker_times.iter().enumerate().skip(1) {
            if *time < min_time {
                min_time = *time;
                idx = i;
            }
        }
        worker_times[idx] = min_time + duration;
    }
    worker_times.into_iter().fold(0f64, f64::max) as u64 + spin
}

/// Benchmarks simulated processing time for static vs dynamic chunking.
fn bench_chunking(c: &mut Criterion) {
    let work_units = WORK_UNITS;
    let scenarios = [
        ("static_idle_8", 8, None),
        (
            "dynamic_idle_8",
            8,
            Some(WorkerSnapshot {
                available_accounts: 8,
                available_storage: 6,
                pending_accounts: 0,
                pending_storage: 0,
            }),
        ),
        (
            "dynamic_idle_2",
            2,
            Some(WorkerSnapshot {
                available_accounts: 2,
                available_storage: 1,
                pending_accounts: 0,
                pending_storage: 0,
            }),
        ),
    ];

    let mut group = c.benchmark_group("multiproof_chunking");
    for (label, workers, snapshot) in scenarios {
        let chunk_size = snapshot
            .and_then(|snap| select_dynamic_chunk_size(Some(DEFAULT_BASE_CHUNK), work_units, snap))
            .unwrap_or(DEFAULT_BASE_CHUNK);
        group.bench_with_input(
            BenchmarkId::new("processing_time_ns", label),
            &(chunk_size, workers),
            |b, &(chunk_size, workers)| {
                b.iter(|| black_box(simulate_processing(WORK_UNITS, chunk_size, workers)));
            },
        );
    }
    group.finish();
}

criterion_group!(multiproof_chunking, bench_chunking);
criterion_main!(multiproof_chunking);
