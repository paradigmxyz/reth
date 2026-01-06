#![allow(missing_docs)]

//! Benchmark for storage trie write batching optimization.
//!
//! This benchmark compares write performance using:
//! 1. The optimized implementation (locality-aware batched writes)
//! 2. A baseline that seeks for every update
//!
//! The optimization exploits the fact that storage trie updates are sorted,
//! so we can use O(1) next_dup operations instead of O(log N) seek operations.

use alloy_primitives::B256;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_db_api::{
    cursor::{DbCursorRW, DbDupCursorRO},
    tables,
    transaction::DbTxMut,
};
use reth_provider::test_utils::create_test_provider_factory;
use reth_trie::{
    updates::StorageTrieUpdatesSorted, BranchNodeCompact, Nibbles, StorageTrieEntry,
    StoredNibblesSubKey,
};
use reth_trie_db::DatabaseStorageTrieCursor;
use std::hint::black_box as bb;

/// Number of entries to update for benchmarking
const UPDATE_COUNTS: &[usize] = &[100, 500, 1000];

/// Generate sorted nibbles keys for StoragesTrie table
fn generate_sorted_nibbles_keys(count: usize) -> Vec<Nibbles> {
    let mut keys: Vec<Nibbles> = (0..count as u64)
        .map(|i| {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&i.to_be_bytes());
            Nibbles::unpack(&bytes)
        })
        .collect();
    keys.sort();
    keys
}

/// Create storage trie updates for benchmarking
fn create_storage_updates(keys: &[Nibbles]) -> StorageTrieUpdatesSorted {
    let storage_nodes: Vec<(Nibbles, Option<BranchNodeCompact>)> = keys
        .iter()
        .map(|key| {
            let node = BranchNodeCompact::new(0b1111, 0b0011, 0, vec![], None);
            (*key, Some(node))
        })
        .collect();
    StorageTrieUpdatesSorted { is_deleted: false, storage_nodes }
}

/// Benchmark storage trie write batching optimization
fn bench_storage_trie_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("StoragesTrie Write Batching");

    for &count in UPDATE_COUNTS {
        let keys = generate_sorted_nibbles_keys(count);
        let updates = create_storage_updates(&keys);

        // Benchmark: Optimized batched writes with locality optimization
        group.bench_with_input(
            BenchmarkId::new("optimized_batched_write", count),
            &count,
            |b, _| {
                b.iter(|| {
                    let factory = create_test_provider_factory();
                    let provider = factory.provider_rw().unwrap();
                    let hashed_address = B256::random();

                    // Pre-populate with some existing entries
                    {
                        let mut cursor =
                            provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
                        for key in keys.iter().step_by(2) {
                            let node = BranchNodeCompact::new(0b1010, 0b1010, 0, vec![], None);
                            cursor
                                .upsert(
                                    hashed_address,
                                    &StorageTrieEntry {
                                        nibbles: StoredNibblesSubKey(*key),
                                        node,
                                    },
                                )
                                .unwrap();
                        }
                    }

                    // Perform the optimized write
                    let mut cursor = DatabaseStorageTrieCursor::new(
                        provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap(),
                        hashed_address,
                    );
                    let _ = bb(cursor.write_storage_trie_updates_sorted(bb(&updates)));
                });
            },
        );

        // Benchmark: Baseline that seeks for every update
        group.bench_with_input(
            BenchmarkId::new("baseline_seek_per_update", count),
            &count,
            |b, _| {
                b.iter(|| {
                    let factory = create_test_provider_factory();
                    let provider = factory.provider_rw().unwrap();
                    let hashed_address = B256::random();

                    // Pre-populate with some existing entries
                    {
                        let mut cursor =
                            provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
                        for key in keys.iter().step_by(2) {
                            let node = BranchNodeCompact::new(0b1010, 0b1010, 0, vec![], None);
                            cursor
                                .upsert(
                                    hashed_address,
                                    &StorageTrieEntry {
                                        nibbles: StoredNibblesSubKey(*key),
                                        node,
                                    },
                                )
                                .unwrap();
                        }
                    }

                    // Perform writes with seek per update (baseline)
                    let mut cursor =
                        provider.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

                    for (nibbles, maybe_updated) in
                        updates.storage_nodes.iter().filter(|(n, _)| !n.is_empty())
                    {
                        let target = StoredNibblesSubKey(*nibbles);
                        // Baseline: always seek
                        if cursor
                            .seek_by_key_subkey(hashed_address, target.clone())
                            .unwrap()
                            .filter(|e| e.nibbles == target)
                            .is_some()
                        {
                            cursor.delete_current().unwrap();
                        }

                        if let Some(node) = maybe_updated {
                            cursor
                                .upsert(
                                    hashed_address,
                                    &StorageTrieEntry { nibbles: target, node: node.clone() },
                                )
                                .unwrap();
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_storage_trie_writes);
criterion_main!(benches);
