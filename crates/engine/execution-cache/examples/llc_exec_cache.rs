//! Run: `LLC_BENCH_CPUS=0,1,2,3,4,5 cargo run --release -p reth-execution-cache --example
//! llc_exec_cache`, then again with a spread list such as `LLC_BENCH_CPUS=0,6,12,18,1,7`.
//!
//! Cross-L3 contention microbenchmark for hot [`ExecutionCache`] hits: every bench thread looks up
//! the same 64 cached accounts or storage slots with a little busy work in between, the way
//! prewarm workers and execution hit the cache on every `SLOAD` and account load. Compare a CPU
//! list inside one L3 with one spread across L3 domains.
//!
//! Args: `[ops per round] [rounds]`.

use alloy_primitives::{Address, StorageKey, U256};
use reth_execution_cache::{CachedStatus, ExecutionCache};
use reth_primitives_traits::Account;
use std::{
    hint::black_box,
    sync::{
        atomic::{AtomicU64, Ordering},
        Barrier,
    },
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ops: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let cpus = bench_cpus();

    let cache = ExecutionCache::new(64 << 20);
    let accounts: Vec<Address> = (0..64).map(address).collect();
    let slots: Vec<(Address, StorageKey)> =
        (0..64u64).map(|i| (address(i % 8), StorageKey::from(U256::from(i)))).collect();
    for (i, a) in accounts.iter().enumerate() {
        cache.insert_account(*a, Some(Account::new(i as u64, U256::from(i * 1000), None)));
    }
    for (i, (a, k)) in slots.iter().enumerate() {
        cache.insert_storage(*a, *k, Some(U256::from(i + 1)));
    }

    let misses = AtomicU64::new(0);
    run("exec_cache_storage_hit", &cpus, rounds, ops, |t| {
        let mut rng = Rng(0x9e37_79b9 ^ (t as u64 + 1));
        for _ in 0..ops {
            let (a, k) = slots[(rng.next() % slots.len() as u64) as usize];
            match cache.get_or_try_insert_storage_with(a, k, || Ok::<_, ()>(U256::ZERO)) {
                Ok(CachedStatus::Cached(v)) => {
                    black_box(v);
                }
                _ => {
                    misses.fetch_add(1, Ordering::Relaxed);
                }
            }
            spin_work(rng.0);
        }
    });
    run("exec_cache_account_hit", &cpus, rounds, ops, |t| {
        let mut rng = Rng(0x517c_c1b7 ^ (t as u64 + 1));
        for _ in 0..ops {
            let a = accounts[(rng.next() % accounts.len() as u64) as usize];
            match cache.get_or_try_insert_account_with(a, || Ok::<_, ()>(None)) {
                Ok(CachedStatus::Cached(v)) => {
                    black_box(v);
                }
                _ => {
                    misses.fetch_add(1, Ordering::Relaxed);
                }
            }
            spin_work(rng.0);
        }
    });
    assert_eq!(misses.load(Ordering::Relaxed), 0, "every lookup should hit the cache");
}

fn address(i: u64) -> Address {
    let mut a = [0u8; 20];
    a[..8].copy_from_slice(&i.wrapping_mul(0x9e37_79b9_7f4a_7c15).to_le_bytes());
    a[12..].copy_from_slice(&i.to_be_bytes());
    Address::from(a)
}

/// Twenty dependent multiply-adds, standing in for EVM work between cache accesses.
fn spin_work(seed: u64) -> u64 {
    let mut x = seed | 1;
    for _ in 0..20 {
        x = black_box(x).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    black_box(x)
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
