//! Benchmark for `StateRootTask` complete workflow, including sending state
//! updates using the incoming messages sender and waiting for the final result.

#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::test_runner::TestRunner;
use rand::Rng;
use reth_engine_tree::tree::root::{StateRootConfig, StateRootTask};
use reth_evm::system_calls::OnStateHook;
use reth_primitives::{Account as RethAccount, StorageEntry};
use reth_provider::{
    providers::ConsistentDbView,
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    AccountReader, HashingWriter, ProviderFactory,
};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory, TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use revm_primitives::{
    Account as RevmAccount, AccountInfo, AccountStatus, Address, EvmState, EvmStorageSlot, HashMap,
    B256, KECCAK_EMPTY, U256,
};
use std::hint::black_box;

#[derive(Debug, Clone)]
struct BenchParams {
    num_accounts: usize,
    updates_per_account: usize,
    storage_slots_per_account: usize,
    selfdestructs_per_update: usize,
}

/// Generates a series of random state updates with configurable accounts,
/// storage, and self-destructs
fn create_bench_state_updates(params: &BenchParams) -> Vec<EvmState> {
    let mut runner = TestRunner::deterministic();
    let mut rng = runner.rng().clone();
    let all_addresses: Vec<Address> = (0..params.num_accounts).map(|_| rng.gen()).collect();
    let mut updates = Vec::new();

    for _ in 0..params.updates_per_account {
        let mut state_update = EvmState::default();
        let num_accounts_in_update = rng.gen_range(1..=params.num_accounts);

        // regular updates for randomly selected accounts
        for &address in &all_addresses[0..num_accounts_in_update] {
            // randomly choose to self-destruct with probability
            // (selfdestructs/accounts)
            let is_selfdestruct =
                rng.gen_bool(params.selfdestructs_per_update as f64 / params.num_accounts as f64);

            let account = if is_selfdestruct {
                RevmAccount {
                    info: AccountInfo::default(),
                    storage: HashMap::default(),
                    status: AccountStatus::SelfDestructed,
                }
            } else {
                RevmAccount {
                    info: AccountInfo {
                        balance: U256::from(rng.gen::<u64>()),
                        nonce: rng.gen::<u64>(),
                        code_hash: KECCAK_EMPTY,
                        code: Some(Default::default()),
                    },
                    storage: (0..rng.gen_range(0..=params.storage_slots_per_account))
                        .map(|_| {
                            (
                                U256::from(rng.gen::<u64>()),
                                EvmStorageSlot::new_changed(
                                    U256::ZERO,
                                    U256::from(rng.gen::<u64>()),
                                ),
                            )
                        })
                        .collect(),
                    status: AccountStatus::Touched,
                }
            };

            state_update.insert(address, account);
        }

        updates.push(state_update);
    }

    updates
}

fn convert_revm_to_reth_account(revm_account: &RevmAccount) -> Option<RethAccount> {
    match revm_account.status {
        AccountStatus::SelfDestructed => None,
        _ => Some(RethAccount {
            balance: revm_account.info.balance,
            nonce: revm_account.info.nonce,
            bytecode_hash: if revm_account.info.code_hash == KECCAK_EMPTY {
                None
            } else {
                Some(revm_account.info.code_hash)
            },
        }),
    }
}

/// Applies state updates to the provider, ensuring self-destructs only affect
/// existing accounts
fn setup_provider(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    state_updates: &[EvmState],
) -> Result<(), Box<dyn std::error::Error>> {
    for update in state_updates {
        let provider_rw = factory.provider_rw()?;

        let mut account_updates = Vec::new();

        for (address, account) in update {
            // only process self-destructs if account exists, always process
            // other updates
            let should_process = match account.status {
                AccountStatus::SelfDestructed => {
                    provider_rw.basic_account(address).ok().flatten().is_some()
                }
                _ => true,
            };

            if should_process {
                account_updates.push((
                    *address,
                    convert_revm_to_reth_account(account),
                    (account.status == AccountStatus::Touched).then(|| {
                        account
                            .storage
                            .iter()
                            .map(|(slot, value)| StorageEntry {
                                key: B256::from(*slot),
                                value: value.present_value,
                            })
                            .collect::<Vec<_>>()
                    }),
                ));
            }
        }

        // update in the provider account and its storage (if available)
        for (address, account, maybe_storage) in account_updates {
            provider_rw.insert_account_for_hashing(std::iter::once((address, account)))?;
            if let Some(storage) = maybe_storage {
                provider_rw
                    .insert_storage_for_hashing(std::iter::once((address, storage.into_iter())))?;
            }
        }

        provider_rw.commit()?;
    }

    Ok(())
}

fn bench_state_root(c: &mut Criterion) {
    reth_tracing::init_test_tracing();

    let mut group = c.benchmark_group("state_root");

    let scenarios = vec![
        BenchParams {
            num_accounts: 100,
            updates_per_account: 5,
            storage_slots_per_account: 10,
            selfdestructs_per_update: 2,
        },
        BenchParams {
            num_accounts: 1000,
            updates_per_account: 10,
            storage_slots_per_account: 20,
            selfdestructs_per_update: 5,
        },
        BenchParams {
            num_accounts: 500,
            updates_per_account: 8,
            storage_slots_per_account: 15,
            selfdestructs_per_update: 20,
        },
    ];

    for params in scenarios {
        group.bench_with_input(
            BenchmarkId::new(
                "state_root_task",
                format!(
                    "accounts_{}_updates_{}_slots_{}_selfdestructs_{}",
                    params.num_accounts,
                    params.updates_per_account,
                    params.storage_slots_per_account,
                    params.selfdestructs_per_update
                ),
            ),
            &params,
            |b, params| {
                b.iter_with_setup(
                    || {
                        let factory = create_test_provider_factory();
                        let state_updates = create_bench_state_updates(params);
                        setup_provider(&factory, &state_updates).expect("failed to setup provider");

                        let trie_input = TrieInput::from_state(Default::default());
                        let config = StateRootConfig::new_from_input(
                            ConsistentDbView::new(factory, None),
                            trie_input,
                        );
                        let provider = config.consistent_view.provider_ro().unwrap();
                        let nodes_sorted = config.nodes_sorted.clone();
                        let state_sorted = config.state_sorted.clone();
                        let prefix_sets = config.prefix_sets.clone();
                        let num_threads = std::thread::available_parallelism()
                            .map_or(1, |num| (num.get() / 2).max(1));

                        let state_root_task_pool = rayon::ThreadPoolBuilder::new()
                            .num_threads(num_threads)
                            .thread_name(|i| format!("proof-worker-{}", i))
                            .build()
                            .expect("Failed to create proof worker thread pool");

                        (
                            config,
                            state_updates,
                            provider,
                            nodes_sorted,
                            state_sorted,
                            prefix_sets,
                            state_root_task_pool,
                        )
                    },
                    |(
                        config,
                        state_updates,
                        provider,
                        nodes_sorted,
                        state_sorted,
                        prefix_sets,
                        state_root_task_pool,
                    )| {
                        let blinded_provider_factory = ProofBlindedProviderFactory::new(
                            InMemoryTrieCursorFactory::new(
                                DatabaseTrieCursorFactory::new(provider.tx_ref()),
                                &nodes_sorted,
                            ),
                            HashedPostStateCursorFactory::new(
                                DatabaseHashedCursorFactory::new(provider.tx_ref()),
                                &state_sorted,
                            ),
                            prefix_sets,
                        );

                        black_box(std::thread::scope(|scope| {
                            let task = StateRootTask::new(
                                config,
                                blinded_provider_factory,
                                &state_root_task_pool,
                            );
                            let mut hook = task.state_hook();
                            let handle = task.spawn(scope);

                            for update in state_updates {
                                hook.on_state(&update)
                            }
                            drop(hook);

                            handle.wait_for_result().expect("task failed")
                        }));
                    },
                )
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_state_root);
criterion_main!(benches);
