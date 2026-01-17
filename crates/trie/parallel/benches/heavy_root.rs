//! Heavy benchmarks for parallel state root calculation.
//!
//! Based on #eng-perf profiling showing:
//! - State root calculation is 50-80% of validation time for 3s blocks
//! - Parallel vs sync root has significant delta at scale
//! - Sparse trie updates vs full recalculation trade-offs
//!
//! Run with: cargo bench -p reth-trie-parallel --bench heavy_root

#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use proptest_arbitrary_interop::arb;
use reth_primitives_traits::Account;
use reth_provider::{
    providers::OverlayStateProviderFactory, test_utils::create_test_provider_factory, StateWriter,
    TrieWriter,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, HashedPostState, HashedStorage, StateRoot,
    TrieInput,
};
use reth_trie_db::{ChangesetCache, DatabaseHashedCursorFactory, DatabaseStateRoot};
use reth_trie_parallel::root::ParallelStateRoot;
use std::collections::HashMap;

/// Benchmark parameters for megablock scenarios
#[derive(Debug, Clone)]
struct StateRootParams {
    /// Total accounts in database
    db_accounts: usize,
    /// Storage slots per account
    storage_per_account: usize,
    /// Percentage of accounts updated
    update_percentage: f64,
}

impl StateRootParams {
    fn updated_accounts(&self) -> usize {
        (self.db_accounts as f64 * self.update_percentage) as usize
    }
}

fn generate_heavy_test_data(params: &StateRootParams) -> (HashedPostState, HashedPostState) {
    let mut runner = TestRunner::deterministic();

    let db_state = proptest::collection::hash_map(
        any::<B256>(),
        (
            arb::<Account>().prop_filter("non empty account", |a| !a.is_empty()),
            proptest::collection::hash_map(
                any::<B256>(),
                any::<U256>().prop_filter("non zero value", |v| !v.is_zero()),
                params.storage_per_account,
            ),
        ),
        params.db_accounts,
    )
    .new_tree(&mut runner)
    .unwrap()
    .current();

    let keys = db_state.keys().copied().collect::<Vec<_>>();
    let num_updates = params.updated_accounts();
    let keys_to_update =
        proptest::sample::subsequence(keys, num_updates).new_tree(&mut runner).unwrap().current();

    let updated_storages = keys_to_update
        .into_iter()
        .map(|address| {
            let (_, storage) = db_state.get(&address).unwrap();
            let slots = storage.keys().copied().collect::<Vec<_>>();
            let slots_to_update =
                proptest::sample::subsequence(slots, params.storage_per_account / 2)
                    .new_tree(&mut runner)
                    .unwrap()
                    .current();
            (
                address,
                slots_to_update
                    .into_iter()
                    .map(|slot| (slot, any::<U256>().new_tree(&mut runner).unwrap().current()))
                    .collect::<HashMap<_, _>>(),
            )
        })
        .collect::<HashMap<_, _>>();

    (
        HashedPostState::default()
            .with_accounts(
                db_state.iter().map(|(address, (account, _))| (*address, Some(*account))),
            )
            .with_storages(db_state.into_iter().map(|(address, (_, storage))| {
                (address, HashedStorage::from_iter(false, storage))
            })),
        HashedPostState::default().with_storages(
            updated_storages
                .into_iter()
                .map(|(address, storage)| (address, HashedStorage::from_iter(false, storage))),
        ),
    )
}

/// Benchmark: Sync vs Parallel state root at various scales
fn bench_sync_vs_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_root/sync_vs_parallel");
    group.sample_size(10);

    // Scenarios based on Slack discussions:
    // - Normal block: 3000 accounts
    // - Heavy block: 10000 accounts
    // - Megablock (1.5 GGas): 30000+ accounts
    let scenarios = vec![
        StateRootParams { db_accounts: 3000, storage_per_account: 100, update_percentage: 0.5 },
        StateRootParams { db_accounts: 10000, storage_per_account: 100, update_percentage: 0.3 },
        StateRootParams { db_accounts: 30000, storage_per_account: 50, update_percentage: 0.2 },
    ];

    for params in scenarios {
        let (db_state, updated_state) = generate_heavy_test_data(&params);
        let provider_factory = create_test_provider_factory();

        // Setup: write initial state
        {
            let provider_rw = provider_factory.provider_rw().unwrap();
            provider_rw.write_hashed_state(&db_state.into_sorted()).unwrap();
            let (_, updates) =
                StateRoot::from_tx(provider_rw.tx_ref()).root_with_updates().unwrap();
            provider_rw.write_trie_updates(updates).unwrap();
            provider_rw.commit().unwrap();
        }

        let id = format!("db_{}_updated_{}", params.db_accounts, params.updated_accounts());

        let changeset_cache = ChangesetCache::new();
        let factory = OverlayStateProviderFactory::new(provider_factory.clone(), changeset_cache);

        // Sync state root
        group.bench_function(BenchmarkId::new("sync", &id), |b| {
            b.iter_with_setup(
                || {
                    let sorted_state = updated_state.clone().into_sorted();
                    let prefix_sets = updated_state.construct_prefix_sets().freeze();
                    let provider = provider_factory.provider().unwrap();
                    (provider, sorted_state, prefix_sets)
                },
                |(provider, sorted_state, prefix_sets)| {
                    let hashed_cursor_factory = HashedPostStateCursorFactory::new(
                        DatabaseHashedCursorFactory::new(provider.tx_ref()),
                        &sorted_state,
                    );
                    StateRoot::from_tx(provider.tx_ref())
                        .with_hashed_cursor_factory(hashed_cursor_factory)
                        .with_prefix_sets(prefix_sets)
                        .root()
                },
            );
        });

        // Parallel state root
        group.bench_function(BenchmarkId::new("parallel", &id), |b| {
            b.iter_with_setup(
                || {
                    let trie_input = TrieInput::from_state(updated_state.clone());
                    ParallelStateRoot::new(factory.clone(), trie_input.prefix_sets.freeze())
                },
                |calculator| calculator.incremental_root(),
            );
        });
    }

    group.finish();
}

/// Benchmark: Incremental updates (sparse trie) at scale
fn bench_incremental_updates(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_root/incremental");
    group.sample_size(10);

    // Test repeated incremental updates (simulating back-to-back blocks)
    let num_updates_sequence = [5, 10, 25, 50];

    for num_updates in num_updates_sequence {
        let params =
            StateRootParams { db_accounts: 5000, storage_per_account: 50, update_percentage: 0.1 };

        let id = format!("sequential_updates_{}", num_updates);
        group.throughput(Throughput::Elements(num_updates as u64));

        group.bench_function(BenchmarkId::new("sparse_trie", &id), |b| {
            b.iter_with_setup(
                || {
                    let (db_state, _) = generate_heavy_test_data(&params);
                    let provider_factory = create_test_provider_factory();

                    {
                        let provider_rw = provider_factory.provider_rw().unwrap();
                        provider_rw.write_hashed_state(&db_state.into_sorted()).unwrap();
                        let (_, updates) =
                            StateRoot::from_tx(provider_rw.tx_ref()).root_with_updates().unwrap();
                        provider_rw.write_trie_updates(updates).unwrap();
                        provider_rw.commit().unwrap();
                    }

                    // Generate sequence of updates
                    let updates: Vec<HashedPostState> = (0..num_updates)
                        .map(|_| {
                            let (_, update) = generate_heavy_test_data(&StateRootParams {
                                db_accounts: 500,
                                storage_per_account: 20,
                                update_percentage: 1.0,
                            });
                            update
                        })
                        .collect();

                    (provider_factory, updates)
                },
                |(provider_factory, updates)| {
                    let changeset_cache = ChangesetCache::new();
                    let factory =
                        OverlayStateProviderFactory::new(provider_factory, changeset_cache);

                    let mut roots = Vec::with_capacity(updates.len());
                    for update in updates {
                        let trie_input = TrieInput::from_state(update);
                        let calculator = ParallelStateRoot::new(
                            factory.clone(),
                            trie_input.prefix_sets.freeze(),
                        );
                        roots.push(calculator.incremental_root().unwrap());
                    }
                    roots
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: Large storage trie updates (contract-heavy blocks)
fn bench_large_storage_tries(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_root/large_storage");
    group.sample_size(10);

    // Simulate contracts with large storage (DEX, AMM, etc.)
    let storage_sizes = [1000, 5000, 10000];

    for storage_size in storage_sizes {
        let id = format!("slots_{}", storage_size);
        group.throughput(Throughput::Elements(storage_size as u64));

        group.bench_function(BenchmarkId::new("single_contract", &id), |b| {
            b.iter_with_setup(
                || {
                    let mut runner = TestRunner::deterministic();
                    let contract_address = any::<B256>().new_tree(&mut runner).unwrap().current();

                    let storage: HashMap<B256, U256> = proptest::collection::hash_map(
                        any::<B256>(),
                        any::<U256>().prop_filter("non zero", |v| !v.is_zero()),
                        storage_size,
                    )
                    .new_tree(&mut runner)
                    .unwrap()
                    .current();

                    let db_state = HashedPostState::default()
                        .with_accounts(std::iter::once((
                            contract_address,
                            Some(Account::default()),
                        )))
                        .with_storages(std::iter::once((
                            contract_address,
                            HashedStorage::from_iter(false, storage.clone()),
                        )));

                    let provider_factory = create_test_provider_factory();
                    {
                        let provider_rw = provider_factory.provider_rw().unwrap();
                        provider_rw.write_hashed_state(&db_state.into_sorted()).unwrap();
                        let (_, updates) =
                            StateRoot::from_tx(provider_rw.tx_ref()).root_with_updates().unwrap();
                        provider_rw.write_trie_updates(updates).unwrap();
                        provider_rw.commit().unwrap();
                    }

                    // Update half the storage
                    let update_storage: HashMap<B256, U256> = storage
                        .into_iter()
                        .take(storage_size / 2)
                        .map(|(k, _)| (k, U256::from(999)))
                        .collect();

                    let update_state = HashedPostState::default().with_storages(std::iter::once((
                        contract_address,
                        HashedStorage::from_iter(false, update_storage),
                    )));

                    (provider_factory, update_state)
                },
                |(provider_factory, update_state)| {
                    let changeset_cache = ChangesetCache::new();
                    let factory =
                        OverlayStateProviderFactory::new(provider_factory, changeset_cache);

                    let trie_input = TrieInput::from_state(update_state);
                    let calculator =
                        ParallelStateRoot::new(factory, trie_input.prefix_sets.freeze());
                    calculator.incremental_root()
                },
            );
        });
    }

    group.finish();
}

criterion_group!(
    name = heavy_root;
    config = Criterion::default().significance_level(0.05).sample_size(10);
    targets =
        bench_sync_vs_parallel,
        bench_incremental_updates,
        bench_large_storage_tries
);
criterion_main!(heavy_root);
