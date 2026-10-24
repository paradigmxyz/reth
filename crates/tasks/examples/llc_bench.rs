//! Microbenchmark for LLC-aware job placement in reth-tasks.
//!
//! Each workload hands a working set between a submitting thread and a [`WorkerPool`] the way the
//! engine does (`install_fn` round trips, `broadcast` to long-lived proof-style workers, and
//! `spawn` streams), and reports per-iteration latency. Run it twice, with and without
//! `RETH_CACHE_AFFINITY=0`, in fresh processes, to compare placement.
//!
//! `cargo run --release -p reth-tasks --features rayon --example llc_bench -- <workload> <kib>
//! <threads> <iters>`

use reth_tasks::{affinity, WorkerPool};
use std::{
    hint::black_box,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// A cheap, non-vectorizable mix so the work is bound by memory traffic, not ALU.
#[inline]
const fn mix(x: u64) -> u64 {
    let x = x ^ (x >> 31);
    x.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// Submitter dirties the buffer, a pool job reads and rewrites all of it, the submitter reads
/// it back. Mirrors a driver making mid-sized `install` calls on a working set it owns.
fn install_roundtrip(pool: &WorkerPool, words: usize, iters: usize) -> Vec<Duration> {
    let mut buf = vec![1u64; words];
    let mut out = Vec::with_capacity(iters);
    for it in 0..iters as u64 {
        let start = Instant::now();
        for (i, w) in buf.iter_mut().enumerate() {
            *w = mix(*w ^ it ^ i as u64);
        }
        let b = &mut buf;
        pool.install_fn(|| {
            let mut acc = 0u64;
            for w in b.iter_mut() {
                acc = acc.wrapping_add(*w);
                *w = mix(*w);
            }
            black_box(acc);
        });
        black_box(buf.iter().fold(0u64, |a, w| a ^ w));
        out.push(start.elapsed());
    }
    out
}

/// Long-lived workers (like proof-strg/proof-acct) started with `broadcast` pull jobs from a
/// channel; each job reads a slice of a shared table the submitter rewrote for this "block" and
/// returns a result. Measures the time per block.
fn broadcast_workers(pool: &WorkerPool, words: usize, iters: usize) -> Vec<Duration> {
    let threads = pool.num_threads();
    let table: Vec<AtomicU64> = (0..words).map(|i| AtomicU64::new(i as u64)).collect();
    let (job_tx, job_rx) = crossbeam_channel::unbounded::<Option<(usize, usize)>>();
    let (res_tx, res_rx) = crossbeam_channel::unbounded::<u64>();
    let chunks = threads * 8;
    let chunk = words / chunks;

    // Like reth's proof workers, the broadcast is issued from a helper thread placed in the
    // submitter's domain (a named spawn_blocking thread in reth).
    let domain = affinity::JobDomain::capture();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            domain.run(|| {
                pool.broadcast(threads, |_| {
                    while let Ok(Some((from, to))) = job_rx.recv() {
                        let mut acc = 0u64;
                        for w in &table[from..to] {
                            acc = acc.wrapping_add(mix(w.load(Ordering::Relaxed)));
                        }
                        let _ = res_tx.send(acc);
                    }
                });
            })
        });

        let mut out = Vec::with_capacity(iters);
        for it in 0..iters as u64 {
            let start = Instant::now();
            for (i, w) in table.iter().enumerate() {
                w.store(mix(it ^ i as u64), Ordering::Relaxed);
            }
            for c in 0..chunks {
                job_tx.send(Some((c * chunk, (c + 1) * chunk))).unwrap();
            }
            let mut acc = 0u64;
            for _ in 0..chunks {
                acc ^= res_rx.recv().unwrap();
            }
            black_box(acc);
            out.push(start.elapsed());
        }
        for _ in 0..threads {
            job_tx.send(None).unwrap();
        }
        out
    })
}

/// A submitter streams freshly written 4 KiB buffers to one `spawn`ed consumer through a channel,
/// like the tx-iterator feeding prewarm and execution. Measures the time per batch.
fn spawn_stream(pool: &WorkerPool, words: usize, iters: usize) -> Vec<Duration> {
    const PIECE: usize = 512;
    let pieces = (words / PIECE).max(1);
    let mut out = Vec::with_capacity(iters);
    for it in 0..iters as u64 {
        let start = Instant::now();
        let (tx, rx) = mpsc::sync_channel::<Box<[u64; PIECE]>>(64);
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        pool.spawn(move || {
            let mut acc = 0u64;
            while let Ok(piece) = rx.recv() {
                for w in &*piece {
                    acc = acc.wrapping_add(mix(*w));
                }
            }
            let _ = done_tx.send(acc);
        });
        for p in 0..pieces {
            let mut piece = Box::new([0u64; PIECE]);
            for (i, w) in piece.iter_mut().enumerate() {
                *w = mix(it ^ (p * PIECE + i) as u64);
            }
            tx.send(piece).unwrap();
        }
        drop(tx);
        black_box(done_rx.recv().unwrap());
        out.push(start.elapsed());
    }
    out
}

/// Rayon fan-out inside `install_fn`: the submitter rewrites a buffer, the pool's workers split
/// it with `par_chunks_mut`. Only the entry job is pinned; the other workers stay where they ran.
fn install_par(pool: &WorkerPool, words: usize, iters: usize) -> Vec<Duration> {
    use rayon::prelude::*;
    let gather = std::env::var("LLC_GATHER").is_ok_and(|v| v == "1");
    let mut buf = vec![1u64; words];
    let mut out = Vec::with_capacity(iters);
    for it in 0..iters as u64 {
        let start = Instant::now();
        for (i, w) in buf.iter_mut().enumerate() {
            *w = mix(*w ^ it ^ i as u64);
        }
        if gather {
            pool.broadcast(pool.num_threads(), |_| {});
        }
        let b = &mut buf;
        pool.install_fn(|| {
            b.par_chunks_mut(4096).for_each(|c| {
                for w in c {
                    *w = mix(*w);
                }
            });
        });
        black_box(buf.iter().fold(0u64, |a, w| a ^ w));
        out.push(start.elapsed());
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let workload = args.get(1).map(String::as_str).unwrap_or("install");
    let kib: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4096);
    let threads: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let iters: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(500);
    let words = kib * 1024 / 8;

    let pool = WorkerPool::new(threads, "bench");
    let run = |n| match workload {
        "install" => install_roundtrip(&pool, words, n),
        "broadcast" => broadcast_workers(&pool, words, n),
        "stream" => spawn_stream(&pool, words, n),
        "par" => install_par(&pool, words, n),
        other => panic!("unknown workload {other}"),
    };
    // Warm up: build the pool, fault in pages, let threads settle.
    run(iters / 5 + 1);
    let mut samples = run(iters);
    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let p50 = us(samples[samples.len() / 2]);
    let p90 = us(samples[samples.len() * 9 / 10]);
    let mean = samples.iter().map(|d| us(*d)).sum::<f64>() / samples.len() as f64;
    println!(
        "workload={workload} kib={kib} threads={threads} llc_domains={} p50_us={p50:.1} p90_us={p90:.1} mean_us={mean:.1}",
        affinity::llc_domains()
    );
}
