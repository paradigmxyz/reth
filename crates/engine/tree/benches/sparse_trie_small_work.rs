//! Microbenchmark: serial vs parallel crossover for sparse trie storage operations.
//!
//! The key question: at what (active_tries, work_per_item) does Rayon become worthwhile?
//!
//! We sweep both dimensions to produce a crossover map suitable for threshold selection.

use alloy_primitives::{keccak256, B256};
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use revm_primitives::{hash_map::Entry, B256Map};
use std::{hint::black_box, time::Instant};

/// Active tries sweep — fine granularity around expected crossover.
const ACTIVE_TRIES: [usize; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 24, 28, 32, 48, 64];

/// Simulated per-item work costs in keccak256 iterations.
/// 1 keccak ≈ 200ns, so:
///   1  → ~200ns  (trivial: hashmap-only updates)
///   5  → ~1µs    (light trie update, few nodes)
///  25  → ~5µs    (moderate trie update)
///  50  → ~10µs   (heavy trie update / shallow root)
/// 250  → ~50µs   (large storage trie root computation)
/// 500  → ~100µs  (very large storage trie root)
const WORK_SIZES: [usize; 6] = [1, 5, 25, 50, 250, 500];

const ROUNDS: usize = 5_000;
const TRIALS: usize = 5;

/// Simulates per-trie work by running `n_hashes` keccak256 rounds.
#[inline(never)]
fn simulated_work(seed: u64, n_hashes: usize) -> B256 {
    let mut input = [0u8; 32];
    input[24..].copy_from_slice(&seed.to_be_bytes());
    let mut h = keccak256(input);
    for _ in 1..n_hashes {
        h = keccak256(h);
    }
    h
}

fn main() {
    eprintln!(
        "Sweeping {} active_tries x {} work_sizes x {} trials x {} rounds",
        ACTIVE_TRIES.len(),
        WORK_SIZES.len(),
        TRIALS,
        ROUNDS
    );

    // Warmup rayon pool
    let _: B256 = (0..1024u64)
        .into_par_iter()
        .map(|i| simulated_work(i, 10))
        .reduce(|| B256::ZERO, |a, b| a ^ b);

    println!("work_hashes,active_tries,mode,trial,total_us,us_per_round");

    for &work in &WORK_SIZES {
        for &active in &ACTIVE_TRIES {
            // Serial
            for trial in 0..TRIALS {
                let start = Instant::now();
                let mut acc = B256::ZERO;
                for r in 0..ROUNDS {
                    for i in 0..active {
                        acc ^= simulated_work((r * active + i) as u64, work);
                    }
                }
                black_box(acc);
                let total_us = start.elapsed().as_micros();
                let us_per_round = total_us as f64 / ROUNDS as f64;
                println!("{work},{active},serial,{trial},{total_us},{us_per_round:.2}");
            }

            // Parallel with min_len = active (effectively serial in rayon — measures overhead)
            for trial in 0..TRIALS {
                let start = Instant::now();
                let mut acc = B256::ZERO;
                for r in 0..ROUNDS {
                    let round = (0..active)
                        .into_par_iter()
                        .with_min_len(active)
                        .map(|i| simulated_work((r * active + i) as u64, work))
                        .reduce(|| B256::ZERO, |a, b| a ^ b);
                    acc ^= round;
                }
                black_box(acc);
                let total_us = start.elapsed().as_micros();
                let us_per_round = total_us as f64 / ROUNDS as f64;
                println!("{work},{active},par_noop,{trial},{total_us},{us_per_round:.2}");
            }

            // Parallel with min_len=8 (the production setting)
            for trial in 0..TRIALS {
                let start = Instant::now();
                let mut acc = B256::ZERO;
                for r in 0..ROUNDS {
                    let round = (0..active)
                        .into_par_iter()
                        .with_min_len(8)
                        .map(|i| simulated_work((r * active + i) as u64, work))
                        .reduce(|| B256::ZERO, |a, b| a ^ b);
                    acc ^= round;
                }
                black_box(acc);
                let total_us = start.elapsed().as_micros();
                let us_per_round = total_us as f64 / ROUNDS as f64;
                println!("{work},{active},par_ml8,{trial},{total_us},{us_per_round:.2}");
            }

            // Parallel with min_len=1 (maximum splitting)
            for trial in 0..TRIALS {
                let start = Instant::now();
                let mut acc = B256::ZERO;
                for r in 0..ROUNDS {
                    let round = (0..active)
                        .into_par_iter()
                        .with_min_len(1)
                        .map(|i| simulated_work((r * active + i) as u64, work))
                        .reduce(|| B256::ZERO, |a, b| a ^ b);
                    acc ^= round;
                }
                black_box(acc);
                let total_us = start.elapsed().as_micros();
                let us_per_round = total_us as f64 / ROUNDS as f64;
                println!("{work},{active},par_ml1,{trial},{total_us},{us_per_round:.2}");
            }
        }
    }

    eprintln!("Done.");
}
