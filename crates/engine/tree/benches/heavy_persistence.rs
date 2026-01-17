//! Heavy benchmarks targeting persistence bottlenecks identified in #eng-perf.
//!
//! Key bottlenecks from profiling (Jan 2026):
//! - update_history_indices: 26.0% of persist time
//! - write_trie_updates: 25.4%
//! - write_trie_changesets: 24.2%
//! - write_state: 13.8%
//! - write_hashed_state: 10.6%
//!
//! Run with: cargo bench -p reth-engine-tree --bench heavy_persistence

#![allow(missing_docs)]

use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proptest::test_runner::TestRunner;
use rand::Rng;
use reth_chainspec::ChainSpec;
use reth_db_common::init::init_genesis;
use reth_primitives_traits::Account as RethAccount;
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, HistoryWriter, StateWriter,
    TrieWriter,
};
use reth_trie::{HashedPostState, HashedStorage, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use revm_primitives::HashMap;
use std::sync::Arc;

/// Benchmark parameters simulating realistic block sizes
#[derive(Debug, Clone)]
struct PersistenceParams {
    /// Number of accounts modified per block
    accounts_per_block: usize,
    /// Storage slots modified per account
    storage_slots_per_account: usize,
    /// Number of blocks to accumulate before persistence
    blocks_accumulated: usize,
}

impl PersistenceParams {
    fn total_state_changes(&self) -> usize {
        self.accounts_per_block * self.blocks_accumulated
    }

    fn total_storage_changes(&self) -> usize {
        self.total_state_changes() * self.storage_slots_per_account
    }
}

/// Generate realistic state changes simulating high-TPS block execution
fn generate_state_changes(params: &PersistenceParams) -> Vec<(HashedPostState, Vec<B256>)> {
    let mut runner = TestRunner::deterministic();
    let mut rng = runner.rng().clone();
    let mut blocks = Vec::with_capacity(params.blocks_accumulated);

    for _ in 0..params.blocks_accumulated {
        let mut hashed_state = HashedPostState::default();
        let mut account_addresses = Vec::with_capacity(params.accounts_per_block);

        for _ in 0..params.accounts_per_block {
            let address = Address::random_with(&mut rng);
            let hashed_address = alloy_primitives::keccak256(address);
            account_addresses.push(hashed_address);

            let account = RethAccount {
                balance: U256::from(rng.random::<u64>()),
                nonce: rng.random::<u64>(),
                bytecode_hash: if rng.random_bool(0.1) { Some(B256::random()) } else { None },
            };

            hashed_state =
                hashed_state.with_accounts(std::iter::once((hashed_address, Some(account))));

            let storage: HashMap<B256, U256> = (0..params.storage_slots_per_account)
                .map(|_| (B256::random_with(&mut rng), U256::from(rng.random::<u64>())))
                .collect();

            hashed_state = hashed_state.with_storages(std::iter::once((
                hashed_address,
                HashedStorage::from_iter(false, storage),
            )));
        }

        blocks.push((hashed_state, account_addresses));
    }

    blocks
}

/// Benchmark: write_hashed_state performance with varying state sizes
/// Targets the 10.6% bottleneck
fn bench_write_hashed_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("persistence/write_hashed_state");
    group.sample_size(10);

    // Scenarios from Slack discussions:
    // - Normal block: ~500 accounts, ~10 slots each
    // - Heavy DeFi block: ~2000 accounts, ~50 slots each
    // - Megablock (1.5 GGas): ~5000 accounts, ~100 slots each
    let scenarios = vec![
        PersistenceParams {
            accounts_per_block: 500,
            storage_slots_per_account: 10,
            blocks_accumulated: 1,
        },
        PersistenceParams {
            accounts_per_block: 2000,
            storage_slots_per_account: 50,
            blocks_accumulated: 1,
        },
        PersistenceParams {
            accounts_per_block: 5000,
            storage_slots_per_account: 100,
            blocks_accumulated: 1,
        },
    ];

    for params in scenarios {
        let id = format!(
            "accounts_{}_slots_{}",
            params.accounts_per_block, params.storage_slots_per_account
        );
        group.throughput(Throughput::Elements(params.total_storage_changes() as u64));

        group.bench_function(BenchmarkId::new("single_block", &id), |b| {
            b.iter_with_setup(
                || {
                    let factory = create_test_provider_factory_with_chain_spec(Arc::new(
                        ChainSpec::default(),
                    ));
                    let _ = init_genesis(&factory).unwrap();
                    let blocks = generate_state_changes(&params);
                    (factory, blocks)
                },
                |(factory, blocks)| {
                    let provider_rw = factory.provider_rw().unwrap();
                    for (hashed_state, _) in blocks {
                        provider_rw.write_hashed_state(&hashed_state.into_sorted()).unwrap();
                    }
                    provider_rw.commit().unwrap();
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: Accumulated block persistence (back-to-back scenario)
/// This simulates the O(N²) overlay merge problem identified in Slack
fn bench_accumulated_persistence(c: &mut Criterion) {
    let mut group = c.benchmark_group("persistence/accumulated_blocks");
    group.sample_size(10);

    // Simulate the "backpressure flywheel" problem:
    // Higher throughput → more blocks accumulate → longer persist time
    let scenarios = vec![
        // Normal: ~75 blocks accumulated (from Slack discussion)
        PersistenceParams {
            accounts_per_block: 200,
            storage_slots_per_account: 20,
            blocks_accumulated: 75,
        },
        // Heavy backpressure: ~250 blocks accumulated
        PersistenceParams {
            accounts_per_block: 200,
            storage_slots_per_account: 20,
            blocks_accumulated: 250,
        },
    ];

    for params in scenarios {
        let id =
            format!("blocks_{}_accounts_{}", params.blocks_accumulated, params.accounts_per_block);
        group.throughput(Throughput::Elements(params.blocks_accumulated as u64));

        group.bench_function(BenchmarkId::new("overlay_merge", &id), |b| {
            b.iter_with_setup(
                || {
                    let factory = create_test_provider_factory_with_chain_spec(Arc::new(
                        ChainSpec::default(),
                    ));
                    let _ = init_genesis(&factory).unwrap();
                    let blocks = generate_state_changes(&params);
                    (factory, blocks)
                },
                |(factory, blocks)| {
                    let provider_rw = factory.provider_rw().unwrap();

                    // Simulate merging all overlays - this is where O(N²) happens
                    let mut merged = HashedPostState::default();
                    for (hashed_state, _) in blocks {
                        merged.extend(hashed_state);
                    }

                    provider_rw.write_hashed_state(&merged.into_sorted()).unwrap();
                    provider_rw.commit().unwrap();
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: State root calculation after persistence
/// Targets the 25.4% write_trie_updates + 24.2% write_trie_changesets
fn bench_state_root_after_persist(c: &mut Criterion) {
    let mut group = c.benchmark_group("persistence/state_root");
    group.sample_size(10);

    let scenarios = vec![
        PersistenceParams {
            accounts_per_block: 1000,
            storage_slots_per_account: 20,
            blocks_accumulated: 1,
        },
        PersistenceParams {
            accounts_per_block: 5000,
            storage_slots_per_account: 50,
            blocks_accumulated: 1,
        },
    ];

    for params in scenarios {
        let id = format!(
            "accounts_{}_slots_{}",
            params.accounts_per_block, params.storage_slots_per_account
        );

        group.bench_function(BenchmarkId::new("full_root_calculation", &id), |b| {
            b.iter_with_setup(
                || {
                    let factory = create_test_provider_factory_with_chain_spec(Arc::new(
                        ChainSpec::default(),
                    ));
                    let _ = init_genesis(&factory).unwrap();
                    let blocks = generate_state_changes(&params);

                    // Pre-populate state
                    {
                        let provider_rw = factory.provider_rw().unwrap();
                        for (hashed_state, _) in &blocks {
                            provider_rw
                                .write_hashed_state(&hashed_state.clone().into_sorted())
                                .unwrap();
                        }
                        provider_rw.commit().unwrap();
                    }

                    (factory, blocks)
                },
                |(factory, _blocks)| {
                    let provider = factory.provider().unwrap();
                    let (root, updates) =
                        StateRoot::from_tx(provider.tx_ref()).root_with_updates().unwrap();

                    // Write trie updates - this is where 25.4% of time goes
                    let provider_rw = factory.provider_rw().unwrap();
                    provider_rw.write_trie_updates(updates).unwrap();
                    provider_rw.commit().unwrap();

                    root
                },
            );
        });
    }

    group.finish();
}

/// Benchmark: History indices update performance
/// Targets the 26.0% update_history_indices bottleneck
fn bench_history_indices(c: &mut Criterion) {
    let mut group = c.benchmark_group("persistence/history_indices");
    group.sample_size(10);

    // From Slack: The fix is to derive transitions from in-memory ExecutionOutcome
    // instead of scanning AccountChangeSets/StorageChangeSets tables
    let scenarios = vec![
        PersistenceParams {
            accounts_per_block: 500,
            storage_slots_per_account: 10,
            blocks_accumulated: 10,
        },
        PersistenceParams {
            accounts_per_block: 1000,
            storage_slots_per_account: 20,
            blocks_accumulated: 50,
        },
    ];

    for params in scenarios {
        let id = format!(
            "blocks_{}_accounts_{}_slots_{}",
            params.blocks_accumulated, params.accounts_per_block, params.storage_slots_per_account
        );
        group.throughput(Throughput::Elements(params.total_state_changes() as u64));

        group.bench_function(BenchmarkId::new("insert_indices", &id), |b| {
            b.iter_with_setup(
                || {
                    let factory = create_test_provider_factory_with_chain_spec(Arc::new(
                        ChainSpec::default(),
                    ));
                    let _ = init_genesis(&factory).unwrap();

                    // Build history index data structure (simulating in-memory derivation)
                    // Use Address type for account transitions as required by HistoryWriter
                    let mut account_transitions: std::collections::BTreeMap<Address, Vec<u64>> =
                        std::collections::BTreeMap::new();
                    let mut storage_transitions: std::collections::BTreeMap<
                        (Address, B256),
                        Vec<u64>,
                    > = std::collections::BTreeMap::new();

                    let mut rng = rand::rng();
                    for block_idx in 0..params.blocks_accumulated {
                        let block_number = block_idx as u64 + 1;
                        for _ in 0..params.accounts_per_block {
                            let address = Address::random_with(&mut rng);
                            account_transitions.entry(address).or_default().push(block_number);
                            // Add some storage transitions
                            for i in 0..params.storage_slots_per_account {
                                let slot = B256::from(U256::from(i));
                                storage_transitions
                                    .entry((address, slot))
                                    .or_default()
                                    .push(block_number);
                            }
                        }
                    }

                    (factory, account_transitions, storage_transitions)
                },
                |(factory, account_transitions, storage_transitions)| {
                    let provider_rw = factory.provider_rw().unwrap();

                    // This simulates the optimized path: insert_account_history_index
                    // and insert_storage_history_index from in-memory data
                    provider_rw.insert_account_history_index(account_transitions).unwrap();
                    provider_rw.insert_storage_history_index(storage_transitions).unwrap();

                    provider_rw.commit().unwrap();
                },
            );
        });
    }

    group.finish();
}

criterion_group!(
    name = heavy_persistence;
    config = Criterion::default().significance_level(0.05).sample_size(10);
    targets =
        bench_write_hashed_state,
        bench_accumulated_persistence,
        bench_state_root_after_persist,
        bench_history_indices
);
criterion_main!(heavy_persistence);
