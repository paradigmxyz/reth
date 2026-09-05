#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, U256,
};
use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BatchSize, BenchmarkGroup, BenchmarkId,
    Criterion,
};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie::{
    hashed_cursor::{
        mock::MockHashedCursorFactory, noop::NoopHashedCursorFactory, HashedCursorFactory,
        HashedPostStateCursorFactory,
    },
    proof::StorageProof,
    proof_v2::StorageProofCalculator,
    trie_cursor::{
        mock::MockTrieCursorFactory, noop::NoopTrieCursorFactory, InMemoryTrieCursorFactory,
        TrieCursorFactory,
    },
};
use reth_trie_common::{HashedPostState, HashedStorage, ProofV2Target, ProofV2TargetParent};
use reth_trie_sparse::{ArenaParallelSparseTrie, LeafUpdate, SparseTrie, TrieNodeEpoch};

/// Generate test data for benchmarking.
///
/// Returns a tuple of:
/// - Hashed address for the storage trie
/// - `HashedPostState` with random storage slots
/// - Proof targets as B256 (sorted) for V2 implementation
/// - Equivalent [`B256Set`] for legacy implementation
fn generate_test_data(
    dataset_size: usize,
    num_targets: usize,
) -> (B256, HashedPostState, Vec<B256>, B256Set) {
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
    let hashed_storage =
        HashedStorage { storage: storage_map.iter().map(|(k, v)| (*k, *v)).collect() };
    storages.insert(hashed_address, hashed_storage);

    let hashed_post_state = HashedPostState { accounts: B256Map::default(), storages };

    // Generate proof targets: 80% from existing slots, 20% random
    let mut slot_keys: Vec<B256> = storage_map.keys().copied().collect();
    // Keep target selection identical across runs with different hash map seeds.
    slot_keys.sort_unstable();

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

    // Sort B256 targets for V2 (storage_proof expects sorted targets)
    let mut targets: Vec<B256> = target_b256s.clone();
    targets.sort();

    // Create B256Set for legacy
    let legacy_targets: B256Set = target_b256s.into_iter().collect();

    (hashed_address, hashed_post_state, targets, legacy_targets)
}

/// Create cursor factories from a `HashedPostState` for storage trie testing.
///
/// Cached cases use the branches produced by `StorageRoot` for each storage trie.
fn create_cursor_factories(
    post_state: &HashedPostState,
    cache_branches: bool,
) -> (MockTrieCursorFactory, MockHashedCursorFactory, reth_trie_common::updates::TrieUpdatesSorted)
{
    use reth_trie::{updates::StorageTrieUpdates, StorageRoot};

    let mut trie_updates = reth_trie_common::updates::TrieUpdates {
        storage_tries: post_state
            .storages
            .keys()
            .copied()
            .map(|addr| (addr, StorageTrieUpdates::default()))
            .collect(),
        ..Default::default()
    };
    let hashed_cursor_factory = MockHashedCursorFactory::from_hashed_post_state(post_state.clone());
    if cache_branches {
        let empty_trie_cursor_factory =
            MockTrieCursorFactory::from_trie_updates(trie_updates.clone());
        for (&hashed_address, storage_updates) in &mut trie_updates.storage_tries {
            let (_, _, updates) = StorageRoot::new_hashed(
                empty_trie_cursor_factory.clone(),
                hashed_cursor_factory.clone(),
                hashed_address,
                Default::default(),
                #[cfg(feature = "metrics")]
                reth_trie::metrics::TrieRootMetrics::new(reth_trie::TrieType::Storage),
            )
            .root_with_updates()
            .expect("StorageRoot should succeed");
            *storage_updates = updates;
        }
    }

    let trie_cursor_factory = MockTrieCursorFactory::from_trie_updates(trie_updates.clone());

    (trie_cursor_factory, hashed_cursor_factory, trie_updates.into_sorted())
}

// Benchmark comparing legacy and V2 implementations
fn bench_proof_algos(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proof");
    for dataset_size in [128, 1024, 10240] {
        for num_targets in [1, 16, 64, 128, 512, 2048] {
            let (hashed_address, hashed_post_state, targets, legacy_targets) =
                generate_test_data(dataset_size, num_targets);
            let sorted_state = hashed_post_state.clone().into_sorted();

            for (cache_name, cache_branches) in [("cached", true), ("leaves", false)] {
                let (trie_cursor_factory, hashed_cursor_factory, sorted_updates) =
                    create_cursor_factories(&hashed_post_state, cache_branches);

                let bench_name =
                    format!("{cache_name}/dataset_{dataset_size}/targets_{num_targets}");

                bench_proof_case(
                    &mut group,
                    "Mock",
                    &bench_name,
                    trie_cursor_factory,
                    hashed_cursor_factory,
                    (&targets, &legacy_targets),
                    hashed_address,
                );
                bench_proof_case(
                    &mut group,
                    "Overlay",
                    &bench_name,
                    InMemoryTrieCursorFactory::new(
                        NoopTrieCursorFactory::default(),
                        &sorted_updates,
                    ),
                    HashedPostStateCursorFactory::new(
                        NoopHashedCursorFactory::default(),
                        &sorted_state,
                    ),
                    (&targets, &legacy_targets),
                    hashed_address,
                );
            }
        }
    }
}

fn bench_proof_case<TC: TrieCursorFactory + Clone, HC: HashedCursorFactory + Clone>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    source: &str,
    bench_name: &str,
    trie_cursor_factory: TC,
    hashed_cursor_factory: HC,
    targets: (&[B256], &B256Set),
    hashed_address: B256,
) {
    let (targets, legacy_targets) = targets;
    group.bench_function(BenchmarkId::new(format!("{source}/Legacy"), bench_name), |b| {
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

    for name in ["V2", "V2Partial", "V2Mixed"] {
        group.bench_function(BenchmarkId::new(format!("{source}/{name}"), bench_name), |b| {
            let mut calculator = StorageProofCalculator::new_storage(
                trie_cursor_factory.storage_trie_cursor(hashed_address).unwrap(),
                hashed_cursor_factory.hashed_storage_cursor(hashed_address).unwrap(),
            );
            b.iter_batched(
                || {
                    targets
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(i, key)| {
                            let parent = match name {
                                "V2Partial" => ProofV2TargetParent::new(2),
                                "V2Mixed" => ProofV2TargetParent::new([0, 2, 8][i % 3]),
                                _ => ProofV2TargetParent::NONE,
                            };
                            ProofV2Target::new(key).with_parent(parent)
                        })
                        .collect::<Vec<_>>()
                },
                |mut targets| {
                    calculator.storage_proof(hashed_address, &mut targets).unwrap();
                },
                BatchSize::SmallInput,
            );
        });
    }
    if targets.len() == 1 {
        group.bench_function(BenchmarkId::new(format!("{source}/V2Root"), bench_name), |b| {
            let mut calculator = StorageProofCalculator::new_storage(
                trie_cursor_factory.storage_trie_cursor(hashed_address).unwrap(),
                hashed_cursor_factory.hashed_storage_cursor(hashed_address).unwrap(),
            );
            b.iter(|| calculator.storage_root_node(hashed_address).unwrap());
        });
    }
}

fn bench_sparse_reveal(c: &mut Criterion) {
    let mut group = c.benchmark_group("SparseReveal");
    for size in [1024, 10240] {
        for count in [16, 128, 512] {
            let (address, state, keys, _) = generate_test_data(size, count);
            let (_, _, trie_updates) = create_cursor_factories(&state, true);
            let sorted_state = state.into_sorted();
            let trie_factory =
                InMemoryTrieCursorFactory::new(NoopTrieCursorFactory::default(), &trie_updates);
            let hashed_factory = HashedPostStateCursorFactory::new(
                NoopHashedCursorFactory::default(),
                &sorted_state,
            );
            let mut calculator = StorageProofCalculator::new_storage(
                trie_factory.storage_trie_cursor(address).unwrap(),
                hashed_factory.hashed_storage_cursor(address).unwrap(),
            );
            let root = calculator.storage_root_node(address).unwrap();
            let expected_root =
                calculator.compute_root_hash(std::slice::from_ref(&root)).unwrap().unwrap();
            let mut cached = ArenaParallelSparseTrie::default();
            cached.set_root(root.node, root.masks, false).unwrap();
            let mut warm_targets = sorted_state.storages[&address]
                .storage_slots_ref()
                .iter()
                .step_by(16)
                .map(|(key, _)| ProofV2Target::new(*key))
                .collect::<Vec<_>>();
            let mut nodes = calculator.storage_proof(address, &mut warm_targets).unwrap();
            cached.reveal_nodes(&mut nodes).unwrap();
            assert_eq!(cached.root(TrieNodeEpoch::UNMODIFIED), expected_root);
            let updates =
                keys.into_iter().map(|key| (key, LeafUpdate::Touched)).collect::<B256Map<_>>();

            group.bench_function(
                BenchmarkId::new(format!("dataset_{size}"), format!("targets_{count}")),
                |b| {
                    b.iter_batched(
                        || (cached.clone(), updates.clone()),
                        |(mut trie, mut updates)| {
                            while !updates.is_empty() {
                                let mut targets = Vec::new();
                                trie.update_leaves(&mut updates, |target| targets.push(target))
                                    .unwrap();
                                if targets.is_empty() {
                                    assert!(updates.is_empty());
                                    break
                                }
                                let mut nodes =
                                    calculator.storage_proof(address, &mut targets).unwrap();
                                trie.reveal_nodes(&mut nodes).unwrap();
                            }
                            assert_eq!(trie.root(TrieNodeEpoch::UNMODIFIED), expected_root);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }
}

criterion_group!(proof_comparison, bench_proof_algos, bench_sparse_reveal);
criterion_main!(proof_comparison);
