//! Run: `LLC_BENCH_CPUS=0,1,2,3,4,5 cargo run --release -p reth-trie-sparse --example llc_trie`,
//! then again with a spread list such as `LLC_BENCH_CPUS=0,6,12,18,1,7`.
//!
//! LLC placement microbenchmark on the real sparse trie: one block's worth of leaf updates and a
//! root computation per iteration, run on a `reth_tasks::WorkerPool` through `install_fn`. Both
//! `update_leaves` and `update_subtrie_hashes` fan subtries out with `par_iter` on that pool.
//!
//! Compare `RETH_CACHE_AFFINITY=0` (unplaced), the default (entry job pinned to the submitter's
//! LLC), and `taskset` to one LLC domain (every worker shares the submitter's cache).
//! `LLC_BENCH_CPUS` restricts the pool to those CPUs and pins the submitter to the first.
//!
//! `cargo run --release -p reth-trie-sparse --example llc_trie -- <leaves> <updates> <threads>
//! <iters>`

use alloy_primitives::{map::B256Map, B256};
use reth_tasks::{affinity, WorkerPool};
use reth_trie_common::TrieNodeV2;
use reth_trie_sparse::{ArenaParallelSparseTrie, LeafUpdate, SparseTrie as _, TrieNodeEpoch};
use std::time::{Duration, Instant};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn b256(&mut self) -> B256 {
        let mut b = [0u8; 32];
        for chunk in b.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_le_bytes());
        }
        B256::from(b)
    }
}

fn value(rng: &mut Rng) -> Vec<u8> {
    let mut v = vec![0xa0u8];
    v.extend_from_slice(rng.b256().as_slice());
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let leaves: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(500_000);
    let updates: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let threads: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let iters: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(100);

    // With `LLC_BENCH_CPUS`, pool workers inherit the list as their mask from the thread that
    // builds the pool, and the submitter then runs on the first listed CPU.
    let cpus = bench_cpus();
    if let Some(cpus) = &cpus {
        set_affinity(cpus);
    }
    let pool = WorkerPool::new(threads, "trie");
    if let Some(cpus) = &cpus {
        set_affinity(&cpus[..1]);
    }
    // Before each block, pull every pool worker into the submitter's LLC with a pinned no-op
    // broadcast, so the par_iter splits that follow run there too.
    let gather = std::env::var("LLC_GATHER").is_ok_and(|v| v == "1");
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let keys: Vec<B256> = (0..leaves).map(|_| rng.b256()).collect();

    let mut trie = ArenaParallelSparseTrie::default();
    trie.set_root(TrieNodeV2::EmptyRoot, None, false).unwrap();
    let mut epoch = 1u64;
    for batch in keys.chunks(100_000) {
        let mut map: B256Map<LeafUpdate> =
            batch.iter().map(|k| (*k, LeafUpdate::Changed(value(&mut rng)))).collect();
        pool.install_fn(|| {
            trie.update_leaves(&mut map, |_, _| panic!("no blinded nodes")).unwrap();
            trie.root(TrieNodeEpoch::new(epoch));
        });
        epoch += 1;
    }

    let mut run = |n: usize| {
        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            // The submitter builds the block's updates, as the sparse-trie task does from
            // hashed state, then hands the trie to the pool.
            let mut map: B256Map<LeafUpdate> = (0..updates)
                .map(|_| (keys[rng.next() as usize % leaves], LeafUpdate::Changed(value(&mut rng))))
                .collect();
            let start = Instant::now();
            if gather {
                pool.broadcast(threads, |_| {});
            }
            let trie = &mut trie;
            let root = pool.install_fn(|| {
                trie.update_leaves(&mut map, |_, _| panic!("no blinded nodes")).unwrap();
                trie.root(TrieNodeEpoch::new(epoch))
            });
            std::hint::black_box(root);
            epoch += 1;
            samples.push(start.elapsed());
        }
        samples
    };
    run(iters / 5 + 1);
    let mut samples: Vec<Duration> = run(iters);
    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let mean = samples.iter().map(|d| us(*d)).sum::<f64>() / samples.len() as f64;
    println!(
        "workload=trie leaves={leaves} updates={updates} threads={threads} llc_domains={} p50_us={:.1} p90_us={:.1} mean_us={mean:.1}",
        affinity::llc_domains(),
        us(samples[samples.len() / 2]),
        us(samples[samples.len() * 9 / 10]),
    );
}

/// CPUs listed in `LLC_BENCH_CPUS` (comma-separated), if set.
fn bench_cpus() -> Option<Vec<usize>> {
    let cpus = std::env::var("LLC_BENCH_CPUS").ok()?;
    Some(cpus.split(',').map(|c| c.trim().parse().expect("bad LLC_BENCH_CPUS")).collect())
}

/// Restricts the calling thread to `cpus`. Linux only; elsewhere threads stay unpinned.
fn set_affinity(cpus: &[usize]) {
    #[cfg(target_os = "linux")]
    // SAFETY: an all-zero `cpu_set_t` is the empty set, and pid 0 is the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &cpu in cpus {
            libc::CPU_SET(cpu, &mut set);
        }
        let rc = libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set);
        assert_eq!(rc, 0, "sched_setaffinity({cpus:?}): {}", std::io::Error::last_os_error());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cpus;
}
