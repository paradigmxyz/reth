#![allow(missing_docs)]

use alloy_primitives::{keccak256, Address, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_chain_state::{
    test_utils::TestBlockBuilder, ExecutedBlock, MemoryOverlayStateProviderRef,
};
use reth_ethereum_primitives::EthPrimitives;
use reth_provider::{test_utils::create_test_provider_factory, StateWriter, TrieWriter};
use reth_storage_api::{noop::NoopProvider, BlockHashReader};
use reth_trie::{
    updates::TrieUpdates, HashedPostState, HashedStorage, StateRoot, StorageRoot, TrieInput,
    TrieInputSorted,
};
use reth_trie_db::{DatabaseStateRoot, DatabaseStorageRoot};
use std::collections::HashMap;

criterion_group!(benches, bench_canonical_hashes_range, bench_storage_root_with_cache);
criterion_main!(benches);

fn bench_canonical_hashes_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_hashes_range");

    let scenarios = [("small", 10), ("medium", 100), ("large", 1000)];

    for (name, num_blocks) in scenarios {
        group.bench_function(format!("{}_blocks_{}", name, num_blocks), |b| {
            let (provider, blocks) = setup_provider_with_blocks(num_blocks);
            let start_block = blocks[0].recovered_block().number;
            let end_block = blocks[num_blocks / 2].recovered_block().number;

            b.iter(|| {
                black_box(
                    provider
                        .canonical_hashes_range(black_box(start_block), black_box(end_block))
                        .unwrap(),
                )
            })
        });
    }

    let (provider, blocks) = setup_provider_with_blocks(500);
    let base_block = blocks[100].recovered_block().number;

    let range_sizes = [1, 10, 50, 100, 250];
    for range_size in range_sizes {
        group.bench_function(format!("range_size_{}", range_size), |b| {
            let end_block = base_block + range_size;

            b.iter(|| {
                black_box(
                    provider
                        .canonical_hashes_range(black_box(base_block), black_box(end_block))
                        .unwrap(),
                )
            })
        });
    }

    // Benchmark edge cases
    group.bench_function("no_in_memory_matches", |b| {
        let (provider, blocks) = setup_provider_with_blocks(100);
        let first_block = blocks[0].recovered_block().number;
        let start_block = first_block - 50;
        let end_block = first_block - 10;

        b.iter(|| {
            black_box(
                provider
                    .canonical_hashes_range(black_box(start_block), black_box(end_block))
                    .unwrap(),
            )
        })
    });

    group.bench_function("all_in_memory_matches", |b| {
        let (provider, blocks) = setup_provider_with_blocks(100);
        let first_block = blocks[0].recovered_block().number;
        let last_block = blocks[blocks.len() - 1].recovered_block().number;

        b.iter(|| {
            black_box(
                provider
                    .canonical_hashes_range(black_box(first_block), black_box(last_block + 1))
                    .unwrap(),
            )
        })
    });

    group.finish();
}

fn setup_provider_with_blocks(
    num_blocks: usize,
) -> (MemoryOverlayStateProviderRef<'static, EthPrimitives>, Vec<ExecutedBlock<EthPrimitives>>) {
    let mut builder = TestBlockBuilder::<EthPrimitives>::default();

    let blocks: Vec<_> = builder.get_executed_blocks(1000..1000 + num_blocks as u64).collect();

    let historical = Box::new(NoopProvider::default());
    let provider = MemoryOverlayStateProviderRef::new(historical, blocks.clone());

    (provider, blocks)
}

fn bench_storage_root_with_cache(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_root_with_cache");

    let scenarios = [("small", 10), ("medium", 100), ("large", 1000)];

    for (name, num_slots) in scenarios {
        let provider_factory = create_test_provider_factory();
        let address = Address::random();
        let hashed_address = keccak256(address);

        // Create initial storage with random slots and write to database
        let mut initial_storage_map = HashMap::default();
        for i in 0..num_slots {
            let slot = keccak256(B256::from(U256::from(i)));
            initial_storage_map.insert(slot, U256::from(i + 1));
        }
        let initial_hashed_storage =
            HashedStorage { wiped: false, storage: initial_storage_map.clone() };

        let cached_storage_updates = {
            let provider_rw = provider_factory.provider_rw().unwrap();

            let mut state = HashedPostState::default();
            state.storages.insert(hashed_address, initial_hashed_storage.clone());
            provider_rw.write_hashed_state(&state.into_sorted()).unwrap();

            let (_, updates) =
                StateRoot::from_tx(provider_rw.tx_ref()).root_with_updates().unwrap();
            provider_rw.write_trie_updates(updates).unwrap();

            // Now compute storage root with updates to get cached storage trie nodes
            let (_, _, storage_updates) =
                StorageRoot::from_tx_hashed(provider_rw.tx_ref(), hashed_address)
                    .root_with_updates()
                    .unwrap();

            provider_rw.commit().unwrap();
            storage_updates
        };

        // Create new storage changes to apply on top of existing state
        // This simulates computing a storage root after some slots have changed
        let new_slot = keccak256(B256::from(U256::from(num_slots + 1000)));
        let mut changed_storage_map = HashMap::default();
        changed_storage_map.insert(new_slot, U256::from(999));
        // Also modify an existing slot
        if num_slots > 0 {
            let existing_slot = keccak256(B256::from(U256::from(0)));
            changed_storage_map.insert(existing_slot, U256::from(12345));
        }
        let changed_storage = HashedStorage { wiped: false, storage: changed_storage_map };

        // Build TrieInputSorted with cached nodes
        let cached_trie_input = {
            let mut trie_updates = TrieUpdates::default();
            trie_updates.insert_storage_updates(hashed_address, cached_storage_updates);

            let mut input = TrieInput::from_hashed_storage(hashed_address, changed_storage.clone());
            input.nodes = trie_updates;
            TrieInputSorted::from_unsorted(input)
        };

        group.bench_function(BenchmarkId::new("with_cached_nodes", name), |b| {
            b.iter_with_setup(
                || {
                    let provider = provider_factory.provider().unwrap();
                    (provider, cached_trie_input.clone())
                },
                |(provider, input)| {
                    let result = StorageRoot::overlay_root_from_nodes(
                        provider.tx_ref(),
                        black_box(address),
                        black_box(input),
                    );
                    black_box(result)
                },
            );
        });

        group.bench_function(BenchmarkId::new("without_cached_nodes", name), |b| {
            b.iter_with_setup(
                || {
                    let provider = provider_factory.provider().unwrap();
                    (provider, changed_storage.clone())
                },
                |(provider, storage)| {
                    // This computes from database only - must traverse entire trie
                    let result = StorageRoot::overlay_root(
                        provider.tx_ref(),
                        black_box(address),
                        black_box(storage),
                    );
                    black_box(result)
                },
            );
        });
    }

    group.finish();
}
