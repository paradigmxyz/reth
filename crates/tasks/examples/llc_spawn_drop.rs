//! Run: `LLC_BENCH_CPUS=0,1,2,3,4,5 cargo run --release -p reth-tasks --example llc_spawn_drop`,
//! then again with a spread list such as `LLC_BENCH_CPUS=0,6,12,18,1,7`.
//!
//! Cross-L3 microbenchmark for [`Runtime::spawn_drop`]: every bench thread builds a batch of 512
//! small allocations and hands it to `spawn_drop`, as the engine does with executed blocks, tries
//! and prewarm results. Measures the producer's cost per batch, which rises when the batches are
//! freed on another L3 domain. Compare a CPU list inside one L3 with one spread across domains.
//!
//! Args: `[batches per round] [rounds]`.

use reth_tasks::Runtime;
use std::{
    sync::Barrier,
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ops: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2_000);
    let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let cpus = bench_cpus();

    let runtime = Runtime::test();
    run("spawn_drop", &cpus, rounds, ops, |t| {
        for i in 0..ops {
            let batch: Vec<Box<[u64; 8]>> =
                (0..512).map(|j| Box::new([i ^ j ^ t as u64; 8])).collect();
            runtime.spawn_drop(batch);
        }
    });
}

/// Bench thread CPUs from `LLC_BENCH_CPUS` (thread i on the i-th CPU), or four unpinned threads.
fn bench_cpus() -> Vec<Option<usize>> {
    std::env::var("LLC_BENCH_CPUS").map_or_else(
        |_| vec![None; 4],
        |cpus| {
            cpus.split(',').map(|c| Some(c.trim().parse().expect("bad LLC_BENCH_CPUS"))).collect()
        },
    )
}

/// Pins the calling thread to `cpu`. Linux only; elsewhere threads stay unpinned.
fn pin(cpu: Option<usize>) {
    #[cfg(target_os = "linux")]
    if let Some(cpu) = cpu {
        // SAFETY: an all-zero `cpu_set_t` is the empty set, and pid 0 is the calling thread.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            let rc = libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set);
            assert_eq!(rc, 0, "sched_setaffinity({cpu}): {}", std::io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cpu;
}

/// Runs `work(thread)` (`ops` operations) on every bench thread for `rounds` rounds that start
/// together, after one untimed warm-up, and prints ns/op percentiles and total throughput.
fn run(name: &str, cpus: &[Option<usize>], rounds: usize, ops: u64, work: impl Fn(usize) + Sync) {
    let start = Barrier::new(cpus.len() + 1);
    let end = Barrier::new(cpus.len() + 1);
    let (mut samples, wall) = std::thread::scope(|s| {
        let handles: Vec<_> = cpus
            .iter()
            .enumerate()
            .map(|(i, &cpu)| {
                let (work, start, end) = (&work, &start, &end);
                s.spawn(move || {
                    pin(cpu);
                    work(i);
                    let mut samples = Vec::with_capacity(rounds);
                    for _ in 0..rounds {
                        start.wait();
                        let t = Instant::now();
                        work(i);
                        samples.push(t.elapsed().as_nanos() as f64 / ops as f64);
                        end.wait();
                    }
                    samples
                })
            })
            .collect();
        let mut wall = Duration::ZERO;
        for _ in 0..rounds {
            let t = Instant::now();
            start.wait();
            end.wait();
            wall += t.elapsed();
        }
        (handles.into_iter().flat_map(|h| h.join().unwrap()).collect::<Vec<_>>(), wall)
    });
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let total = ops as f64 * rounds as f64 * cpus.len() as f64;
    println!(
        "bench={name} threads={} cpus={} p50_ns={:.1} p90_ns={:.1} mean_ns={mean:.1} ops_per_s={:.0}",
        cpus.len(),
        std::env::var("LLC_BENCH_CPUS").unwrap_or_else(|_| "unpinned".into()),
        samples[samples.len() / 2],
        samples[samples.len() * 9 / 10],
        total / wall.as_secs_f64(),
    );
}
