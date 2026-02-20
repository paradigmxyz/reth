//! Microbenchmark for sparse trie small-work scheduler overhead and Rayon split tuning.

use alloy_primitives::{keccak256, B256};
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use revm_primitives::{hash_map::Entry, B256Map};
use std::{hint::black_box, time::Instant};

const ACTIVE_TRIES: [usize; 8] = [1, 2, 4, 8, 12, 16, 24, 32];
const MIN_LENS: [usize; 4] = [1, 2, 4, 8];
const ROUNDS: usize = 10_000;

fn leaf_update_unit(seed: u64) -> usize {
    let mut fetched = B256Map::<u8>::default();
    for i in 0..4u64 {
        let mut key_bytes = [0u8; 32];
        key_bytes[24..].copy_from_slice(&(seed.wrapping_mul(13).wrapping_add(i)).to_be_bytes());
        let key = B256::from(key_bytes);

        match fetched.entry(key) {
            Entry::Occupied(mut entry) => {
                let next = entry.get().saturating_sub(1);
                entry.insert(next);
            }
            Entry::Vacant(entry) => {
                entry.insert(8);
            }
        }
    }
    fetched.len()
}

fn root_unit(seed: u64) -> B256 {
    let mut input = [0u8; 32];
    input[24..].copy_from_slice(&seed.to_be_bytes());
    keccak256(input)
}

fn main() {
    println!("kind,active_tries,mode,min_len,total_ns");

    for active in ACTIVE_TRIES {
        let serial_start = Instant::now();
        let mut serial_acc = 0usize;
        for r in 0..ROUNDS {
            serial_acc = serial_acc.wrapping_add(
                (0..active).map(|i| leaf_update_unit((r * active + i) as u64)).sum::<usize>(),
            );
        }
        black_box(serial_acc);
        println!("leaf_updates,{active},serial,0,{}", serial_start.elapsed().as_nanos());

        for min_len in MIN_LENS {
            let par_start = Instant::now();
            let mut par_acc = 0usize;
            for r in 0..ROUNDS {
                par_acc = par_acc.wrapping_add(
                    (0..active)
                        .into_par_iter()
                        .with_min_len(min_len)
                        .map(|i| leaf_update_unit((r * active + i) as u64))
                        .sum::<usize>(),
                );
            }
            black_box(par_acc);
            println!("leaf_updates,{active},parallel,{min_len},{}", par_start.elapsed().as_nanos());
        }
    }

    for active in ACTIVE_TRIES {
        let serial_start = Instant::now();
        let mut serial_acc = B256::ZERO;
        for r in 0..ROUNDS {
            for i in 0..active {
                serial_acc ^= root_unit((r * active + i) as u64);
            }
        }
        black_box(serial_acc);
        println!("storage_roots,{active},serial,0,{}", serial_start.elapsed().as_nanos());

        for min_len in MIN_LENS {
            let par_start = Instant::now();
            let mut par_acc = B256::ZERO;
            for r in 0..ROUNDS {
                let round_hash = (0..active)
                    .into_par_iter()
                    .with_min_len(min_len)
                    .map(|i| root_unit((r * active + i) as u64))
                    .reduce(|| B256::ZERO, |a, b| a ^ b);
                par_acc ^= round_hash;
            }
            black_box(par_acc);
            println!(
                "storage_roots,{active},parallel,{min_len},{}",
                par_start.elapsed().as_nanos()
            );
        }
    }
}
