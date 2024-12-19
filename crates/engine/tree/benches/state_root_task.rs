//! Benchmark for `StateRootTask` complete workflow, including sending state
//! updates using the incoming messages sender and waiting for the final result.

#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_engine_tree::tree::root::{StateRootConfig, StateRootTask};
use reth_evm::system_calls::OnStateHook;
use reth_primitives::{Account as RethAccount, StorageEntry};
use reth_provider::{
    providers::ConsistentDbView,
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    HashingWriter, ProviderFactory,
};
use reth_testing_utils::generators::{self, Rng};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory, TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use revm_primitives::{
    Account as RevmAccount, AccountInfo, AccountStatus, Address, EvmState, EvmStorageSlot, HashMap,
    B256, KECCAK_EMPTY, U256,
};

#[derive(Debug, Clone)]
struct BenchParams {
    num_accounts: usize,
    updates_per_account: usize,
    storage_slots_per_account: usize,
}

fn create_bench_state_updates(params: &BenchParams) -> Vec<EvmState> {
    let mut rng = generators::rng();
    let all_addresses: Vec<Address> = (0..params.num_accounts).map(|_| rng.gen()).collect();
    let mut updates = Vec::new();

    for _ in 0..params.updates_per_account {
        let num_accounts_in_update = rng.gen_range(1..=params.num_accounts);
        let mut state_update = EvmState::default();

        let selected_addresses = &all_addresses[0..num_accounts_in_update];

        for &address in selected_addresses {
            let mut storage = HashMap::default();
            for _ in 0..params.storage_slots_per_account {
                let slot = U256::from(rng.gen::<u64>());
                storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(U256::ZERO, U256::from(rng.gen::<u64>())),
                );
            }

            let account = RevmAccount {
                info: AccountInfo {
                    balance: U256::from(rng.gen::<u64>()),
                    nonce: rng.gen::<u64>(),
                    code_hash: KECCAK_EMPTY,
                    code: Some(Default::default()),
                },
                storage,
                status: AccountStatus::Touched,
            };

            state_update.insert(address, account);
        }

        updates.push(state_update);
    }

    updates
}

fn convert_revm_to_reth_account(revm_account: &RevmAccount) -> RethAccount {
    RethAccount {
        balance: revm_account.info.balance,
        nonce: revm_account.info.nonce,
        bytecode_hash: if revm_account.info.code_hash == KECCAK_EMPTY {
            None
        } else {
            Some(revm_account.info.code_hash)
        },
    }
}

fn setup_provider(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    state_updates: &[EvmState],
) -> Result<(), Box<dyn std::error::Error>> {
    let provider_rw = factory.provider_rw()?;

    for update in state_updates {
        let account_updates = update
            .iter()
            .map(|(address, account)| (*address, Some(convert_revm_to_reth_account(account))));
        provider_rw.insert_account_for_hashing(account_updates)?;

        let storage_updates = update.iter().map(|(address, account)| {
            let storage_entries = account.storage.iter().map(|(slot, value)| StorageEntry {
                key: B256::from(*slot),
                value: value.present_value,
            });
            (*address, storage_entries)
        });
        provider_rw.insert_storage_for_hashing(storage_updates)?;
    }

    provider_rw.commit()?;
    Ok(())
}

fn bench_state_root(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_root");

    let scenarios = vec![
        BenchParams { num_accounts: 100, updates_per_account: 5, storage_slots_per_account: 10 },
        BenchParams { num_accounts: 1000, updates_per_account: 10, storage_slots_per_account: 20 },
    ];

    for params in scenarios {
        group.bench_with_input(
            BenchmarkId::new(
                "state_root_task",
                format!(
                    "accounts_{}_updates_{}_slots_{}",
                    params.num_accounts,
                    params.updates_per_account,
                    params.storage_slots_per_account
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
