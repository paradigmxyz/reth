#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, U256,
};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie::{
    hashed_cursor::{mock::MockHashedCursorFactory, HashedCursorFactory},
    proof::StorageProof,
    proof_v2::StorageProofCalculator,
    trie_cursor::{mock::MockTrieCursorFactory, TrieCursorFactory},
};
use reth_trie_common::{HashedPostState, HashedStorage, Nibbles};
use std::collections::BTreeMap;

/// Generate test data for benchmarking.
///
/// Returns a tuple of:
/// - Hashed address for the storage trie
/// - `HashedPostState` with random storage slots
/// - Proof targets (Nibbles) that are 80% from existing slots, 20% random
/// - Equivalent [`B256Set`] for legacy implementation
fn generate_test_data(
    dataset_size: usize,
    num_targets: usize,
) -> (B256, HashedPostState, Vec<Nibbles>, B256Set) {
    let mut runner = TestRunner::deterministic();

    // Use a fixed hashed address for the storage trie
    let hashed_address = B256::from([0x42; 32]);

    // Generate random storage slots (key -> value)
    let storage_strategy =
        proptest::collection::vec((any::<[u8; 32]>(), any::<u64>()), dataset_size);

    let storage_entries = storage_strategy.new_tree(&mut runner).unwrap().current();

    // Convert to storage map
    let storage_map: B256Map<U256> = storage_entries
        .iter()
        .map(|(slot_bytes, value)| (B256::from(*slot_bytes), U256::from(*value)))
        .collect();

    // Create HashedPostState with single account's storage
    let mut storages = B256Map::default();
    let hashed_storage = HashedStorage {
        wiped: false,
        storage: storage_map.iter().map(|(k, v)| (*k, *v)).collect(),
    };
    storages.insert(hashed_address, hashed_storage);

    let hashed_post_state = HashedPostState { accounts: B256Map::default(), storages };

    // Generate proof targets: 80% from existing slots, 20% random
    let slot_keys: Vec<B256> = storage_map.keys().copied().collect();

    let targets_strategy = proptest::collection::vec(
        prop::bool::weighted(0.8).prop_flat_map(move |from_slots| {
            if from_slots && !slot_keys.is_empty() {
                prop::sample::select(slot_keys.clone()).boxed()
            } else {
                any::<[u8; 32]>().prop_map(B256::from).boxed()
            }
        }),
        num_targets,
    );

    let target_b256s = targets_strategy.new_tree(&mut runner).unwrap().current();

    // Convert B256 targets to sorted Nibbles for V2
    let mut targets: Vec<Nibbles> = target_b256s
        .iter()
        .map(|b256| {
            // SAFETY: B256 is exactly 32 bytes
            unsafe { Nibbles::unpack_unchecked(b256.as_slice()) }
        })
        .collect();
    targets.sort();

    // Create B256Set for legacy
    let legacy_targets: B256Set = target_b256s.into_iter().collect();

    (hashed_address, hashed_post_state, targets, legacy_targets)
}

/// Create cursor factories from a `HashedPostState` for storage trie testing.
///
/// This mimics the test harness pattern from the proof_v2 tests.
fn create_cursor_factories(
    post_state: &HashedPostState,
) -> (MockTrieCursorFactory, MockHashedCursorFactory) {
    // Ensure that there's a storage trie dataset for every storage trie, even if empty
    let storage_trie_nodes: B256Map<BTreeMap<_, _>> =
        post_state.storages.keys().copied().map(|addr| (addr, Default::default())).collect();

    // Create mock hashed cursor factory from the post state
    let hashed_cursor_factory = MockHashedCursorFactory::from_hashed_post_state(post_state.clone());

    // Create empty trie cursor factory (leaf-only calculator doesn't need trie nodes)
    let trie_cursor_factory = MockTrieCursorFactory::new(BTreeMap::new(), storage_trie_nodes);

    (trie_cursor_factory, hashed_cursor_factory)
}

// Benchmark comparing legacy and V2 implementations
fn bench_proof_algos(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proof");
    for dataset_size in [128, 1024, 10240] {
        for num_targets in [1, 16, 64, 128, 512, 2048] {
            let (hashed_address, hashed_post_state, targets, legacy_targets) =
                generate_test_data(dataset_size, num_targets);

            // Create mock cursor factories from the hashed post state
            let (trie_cursor_factory, hashed_cursor_factory) =
                create_cursor_factories(&hashed_post_state);

            let bench_name = format!("dataset_{dataset_size}/targets_{num_targets}");

            group.bench_function(BenchmarkId::new("Legacy", &bench_name), |b| {
                b.iter_batched(
                    || legacy_targets.clone(),
                    |targets| {
                        StorageProof::new_hashed(
                            trie_cursor_factory.clone(),
                            hashed_cursor_factory.clone(),
                            hashed_address,
                        )
                        .storage_multiproof(targets)
                        .expect("Legacy proof generation failed");
                    },
                    BatchSize::SmallInput,
                );
            });

            group.bench_function(BenchmarkId::new("V2", &bench_name), |b| {
                let trie_cursor = trie_cursor_factory
                    .storage_trie_cursor(hashed_address)
                    .expect("Failed to create trie cursor");
                let hashed_cursor = hashed_cursor_factory
                    .hashed_storage_cursor(hashed_address)
                    .expect("Failed to create hashed cursor");

                let mut proof_calculator =
                    StorageProofCalculator::new_storage(trie_cursor, hashed_cursor);

                b.iter_batched(
                    || targets.clone(),
                    |targets| {
                        proof_calculator
                            .storage_proof(hashed_address, targets.into_iter())
                            .expect("Proof generation failed");
                    },
                    BatchSize::SmallInput,
                );
            });
        }
    }
}

criterion_group!(proof_comparison, bench_proof_algos);
criterion_main!(proof_comparison);
