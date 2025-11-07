#![allow(missing_docs)]

use alloy_primitives::{map::B256Set, B256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_engine_tree::tree::{select_dynamic_chunk_size, WorkerSnapshot};
use reth_trie::MultiProofTargets;

const DEFAULT_BASE_CHUNK: usize = 10;

fn make_targets(accounts: usize, slots: usize) -> MultiProofTargets {
    let mut targets = MultiProofTargets::default();
    for account in 0..accounts {
        let hashed = make_hash(account as u64);
        let mut storage = B256Set::default();
        for slot in 0..slots {
            storage.insert(make_hash((account * slots + slot) as u64));
        }
        targets.insert(hashed, storage);
    }
    targets
}

fn make_hash(value: u64) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    B256::from(bytes)
}

fn count_chunks(targets: &MultiProofTargets, chunk_size: usize) -> usize {
    targets.clone().chunks(chunk_size).count()
}

fn bench_chunking(c: &mut Criterion) {
    let targets = make_targets(512, 4);
    let work_units = targets.chunking_length();

    let scenarios = [
        ("static", None),
        (
            "dynamic_idle_8",
            select_dynamic_chunk_size(
                Some(DEFAULT_BASE_CHUNK),
                work_units,
                WorkerSnapshot {
                    available_accounts: 8,
                    available_storage: 6,
                    pending_accounts: 0,
                    pending_storage: 0,
                },
            ),
        ),
        (
            "dynamic_idle_2",
            select_dynamic_chunk_size(
                Some(DEFAULT_BASE_CHUNK),
                work_units,
                WorkerSnapshot {
                    available_accounts: 2,
                    available_storage: 1,
                    pending_accounts: 0,
                    pending_storage: 0,
                },
            ),
        ),
    ];

    let mut group = c.benchmark_group("multiproof_chunking");
    for (label, dynamic_size) in scenarios {
        match dynamic_size {
            Some(size) => {
                group.bench_with_input(BenchmarkId::new("dynamic", label), &size, |b, size| {
                    b.iter(|| count_chunks(&targets, *size));
                });
            }
            None => {
                group.bench_function(BenchmarkId::new("static", label), |b| {
                    b.iter(|| count_chunks(&targets, DEFAULT_BASE_CHUNK));
                });
            }
        }
    }
    group.finish();
}

criterion_group!(multiproof_chunking, bench_chunking);
criterion_main!(multiproof_chunking);
