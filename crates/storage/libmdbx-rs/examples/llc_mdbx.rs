//! Run: `LLC_BENCH_CPUS=0,1,2,3,4,5 cargo run --release -p reth-libmdbx --example llc_mdbx`, then
//! again with a spread list such as `LLC_BENCH_CPUS=0,6,12,18,1,7`.
//!
//! Cross-L3 contention microbenchmark for MDBX read transactions: every bench thread begins a
//! read-only transaction, does one point read and drops it, as every prewarm worker and RPC state
//! provider does, on a shared temporary environment. Compare a CPU list inside one L3 with one
//! spread across L3 domains.
//!
//! Args: `[ops per round] [rounds]`.

use reth_libmdbx::{Environment, Geometry, WriteFlags};
use std::{
    hint::black_box,
    sync::Barrier,
    time::{Duration, Instant},
};

const KEYS: u32 = 1024;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ops: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let cpus = bench_cpus();

    let dir = tempfile::tempdir().unwrap();
    let env = Environment::builder()
        .set_max_readers(1024)
        .set_geometry(Geometry { size: Some(0..(64 << 20)), ..Default::default() })
        .open(dir.path())
        .unwrap();
    let tx = env.begin_rw_txn().unwrap();
    let db = tx.open_db(None).unwrap();
    for i in 0..KEYS {
        tx.put(db.dbi(), i.to_be_bytes(), [7u8; 32], WriteFlags::empty()).unwrap();
    }
    tx.commit().unwrap();

    run("mdbx_ro_txn", &cpus, rounds, ops, |t| {
        let mut rng = Rng(0xdead_beef ^ (t as u64 + 1));
        for _ in 0..ops {
            let txn = env.begin_ro_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            let key = ((rng.next() % KEYS as u64) as u32).to_be_bytes();
            let value: Option<[u8; 32]> = txn.get(db.dbi(), &key).unwrap();
            black_box(value);
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

/// Deterministic xorshift RNG.
struct Rng(u64);

impl Rng {
    const fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
