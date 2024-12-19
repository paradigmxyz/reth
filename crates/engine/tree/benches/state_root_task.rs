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
    AccountReader, HashingWriter, ProviderFactory,
};
use reth_testing_utils::generators::{self, Rng};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory, TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use revm_primitives::{
    alloy_primitives::private::rand::seq::IteratorRandom, Account as RevmAccount, AccountInfo,
    AccountStatus, Address, EvmState, EvmStorageSlot, HashMap, B256, KECCAK_EMPTY, U256,
};
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct BenchParams {
    num_accounts: usize,
    updates_per_account: usize,
    storage_slots_per_account: usize,
    selfdestructs_per_update: usize,
}

fn create_bench_state_updates(params: &BenchParams) -> Vec<EvmState> {
    let mut rng = generators::rng();
    let all_addresses: Vec<Address> = (0..params.num_accounts).map(|_| rng.gen()).collect();
    let mut updates = Vec::new();

    let mut created_accounts: HashSet<Address> = HashSet::new();

    for update_idx in 0..params.updates_per_account {
        let num_accounts_in_update = rng.gen_range(1..=params.num_accounts);
        let mut state_update = EvmState::default();

        let selected_addresses = &all_addresses[0..num_accounts_in_update];

        // first, create/update regular accounts
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
            created_accounts.insert(address);
        }

        // handle self-destructs only for previously created accounts
        if update_idx > 0 && params.selfdestructs_per_update > 0 {
            let available_accounts: Vec<_> = created_accounts.iter().copied().collect();
            let num_selfdestructs = params.selfdestructs_per_update.min(available_accounts.len());

            if num_selfdestructs > 0 {
                let selfdestruct_addresses: Vec<Address> = (0..available_accounts.len())
                    .choose_multiple(&mut rng, num_selfdestructs)
                    .into_iter()
                    .map(|idx| available_accounts[idx])
                    .collect();

                for address in selfdestruct_addresses {
                    let account = RevmAccount {
                        info: AccountInfo {
                            balance: U256::ZERO,
                            nonce: 0,
                            code_hash: KECCAK_EMPTY,
                            code: Some(Default::default()),
                        },
                        storage: HashMap::default(),
                        status: AccountStatus::SelfDestructed,
                    };

                    state_update.insert(address, account);
                    created_accounts.remove(&address);
                }
            }
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

fn setup_provider(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    state_updates: &[EvmState],
) -> Result<(), Box<dyn std::error::Error>> {
    for update in state_updates {
        let provider_rw = factory.provider_rw()?;

        // first pass: Handle account creations and updates (no self-destructs)
        let mut account_updates: Vec<(Address, Option<RethAccount>)> = Vec::new();
        let mut storage_updates: Vec<(Address, Vec<StorageEntry>)> = Vec::new();

        for (address, account) in update.iter() {
            if account.status != AccountStatus::SelfDestructed {
                account_updates.push((*address, convert_revm_to_reth_account(account)));

                let storage_entries: Vec<_> = account
                    .storage
                    .iter()
                    .map(|(slot, value)| StorageEntry {
                        key: B256::from(*slot),
                        value: value.present_value,
                    })
                    .collect();

                if !storage_entries.is_empty() {
                    storage_updates.push((*address, storage_entries));
                }
            }
        }

        // apply regular updates first
        if !account_updates.is_empty() {
            provider_rw.insert_account_for_hashing(account_updates.into_iter())?;
        }

        if !storage_updates.is_empty() {
            provider_rw.insert_storage_for_hashing(
                storage_updates.into_iter().map(|(addr, entries)| (addr, entries.into_iter())),
            )?;
        }

        // second pass: Handle self-destructs
        let mut selfdestruct_updates = Vec::new();

        for (address, account) in update.iter() {
            if account.status == AccountStatus::SelfDestructed {
                // check if account exists in the current state before self-destructing
                if let Ok(Some(_)) = provider_rw.basic_account(*address) {
                    selfdestruct_updates.push((*address, None));
                }
            }
        }

        // apply self-destructs if any
        if !selfdestruct_updates.is_empty() {
            provider_rw.insert_account_for_hashing(selfdestruct_updates.into_iter())?;
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
