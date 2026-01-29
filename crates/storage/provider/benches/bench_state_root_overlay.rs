//! Benchmark comparing MDBX and TrieDB state root calculation performance.
//!
//! Run with:
//! ```
//! cargo bench -p reth-provider --features "test-utils,triedb" --bench bench_state_root_overlay
//! ```

use alloy_primitives::{keccak256, Address, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chainspec::MAINNET;
use reth_primitives_traits::Account;
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, DatabaseProviderFactory,
    HashingWriter, StateRootProvider,
};
use reth_storage_api::PlainPostState;
use reth_trie::{HashedPostState, HashedStorage};
use std::collections::HashMap;

const SEED: u64 = 42;

/// Generate random accounts and storage for benchmarking
fn generate_test_data(
    num_accounts: usize,
    storage_per_account: usize,
    seed: u64,
) -> (Vec<(Address, Account)>, HashMap<Address, Vec<(B256, U256)>>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut accounts = Vec::with_capacity(num_accounts);
    let mut storage = HashMap::new();

    for _ in 0..num_accounts {
        let mut addr_bytes = [0u8; 20];
        rng.fill(&mut addr_bytes);
        let address = Address::from_slice(&addr_bytes);

        let account = Account {
            nonce: rng.random_range(0..1000),
            balance: U256::from(rng.random_range(0u64..1_000_000)),
            bytecode_hash: if rng.random_bool(0.3) {
                let mut hash = [0u8; 32];
                rng.fill(&mut hash);
                Some(B256::from(hash))
            } else {
                None
            },
        };
        accounts.push((address, account));

        // Generate storage for some accounts
        if storage_per_account > 0 && rng.random_bool(0.5) {
            let mut slots = Vec::with_capacity(storage_per_account);
            for _ in 0..storage_per_account {
                let mut key = [0u8; 32];
                rng.fill(&mut key);
                let value = U256::from(rng.random_range(1u64..1_000_000));
                slots.push((B256::from(key), value));
            }
            storage.insert(address, slots);
        }
    }

    (accounts, storage)
}

/// Setup test database with initial state
fn setup_database(
    accounts: &[(Address, Account)],
    storage: &HashMap<Address, Vec<(B256, U256)>>,
) -> reth_provider::providers::ProviderFactory<reth_provider::test_utils::MockNodeTypesWithDB> {
    let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

    {
        let provider_rw = provider_factory.provider_rw().unwrap();

        // Insert accounts
        let accounts_iter = accounts.iter().map(|(addr, acc)| (*addr, Some(*acc)));
        provider_rw.insert_account_for_hashing(accounts_iter).unwrap();

        // Insert storage
        let storage_entries: Vec<_> = storage
            .iter()
            .map(|(addr, slots)| {
                let entries: Vec<_> = slots
                    .iter()
                    .map(|(key, value)| reth_primitives_traits::StorageEntry {
                        key: *key,
                        value: *value,
                    })
                    .collect();
                (*addr, entries)
            })
            .collect();
        provider_rw.insert_storage_for_hashing(storage_entries).unwrap();

        provider_rw.commit().unwrap();
    }

    provider_factory
}

/// Generate overlay state for benchmarking (updates to existing state)
fn generate_overlay(
    base_accounts: &[(Address, Account)],
    overlay_size: usize,
    seed: u64,
) -> (Vec<(Address, Account)>, HashMap<Address, Vec<(B256, U256)>>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut accounts = Vec::with_capacity(overlay_size);
    let mut storage = HashMap::new();

    for i in 0..overlay_size {
        // Mix of existing and new addresses
        let address = if i < base_accounts.len() && rng.random_bool(0.7) {
            base_accounts[i % base_accounts.len()].0
        } else {
            let mut addr_bytes = [0u8; 20];
            rng.fill(&mut addr_bytes);
            Address::from_slice(&addr_bytes)
        };

        let account = Account {
            nonce: rng.random_range(1000..2000),
            balance: U256::from(rng.random_range(1_000_000u64..2_000_000)),
            bytecode_hash: None,
        };
        accounts.push((address, account));

        // Add some storage updates
        if rng.random_bool(0.3) {
            let mut slots = Vec::new();
            for _ in 0..rng.random_range(1..5) {
                let mut key = [0u8; 32];
                rng.fill(&mut key);
                let value = U256::from(rng.random_range(1u64..1_000_000));
                slots.push((B256::from(key), value));
            }
            storage.insert(address, slots);
        }
    }

    (accounts, storage)
}

/// Convert overlay to HashedPostState for MDBX
fn to_hashed_post_state(
    accounts: &[(Address, Account)],
    storage: &HashMap<Address, Vec<(B256, U256)>>,
) -> HashedPostState {
    let hashed_accounts: Vec<_> =
        accounts.iter().map(|(addr, acc)| (keccak256(addr), Some(*acc))).collect();

    let mut hashed_storages = alloy_primitives::map::HashMap::default();
    for (addr, slots) in storage {
        let hashed_addr = keccak256(addr);
        let hashed_storage = HashedStorage::from_iter(
            false,
            slots.iter().map(|(key, value)| (keccak256(key), *value)),
        );
        hashed_storages.insert(hashed_addr, hashed_storage);
    }

    HashedPostState { accounts: hashed_accounts.into_iter().collect(), storages: hashed_storages }
}

/// Convert overlay to PlainPostState for TrieDB
#[cfg(feature = "triedb")]
fn to_plain_post_state(
    accounts: &[(Address, Account)],
    storage: &HashMap<Address, Vec<(B256, U256)>>,
) -> PlainPostState {
    let mut plain_state = PlainPostState::default();

    for (addr, acc) in accounts {
        plain_state.accounts.insert(*addr, Some(*acc));
    }

    for (addr, slots) in storage {
        let mut slot_map = alloy_primitives::map::HashMap::default();
        for (key, value) in slots {
            slot_map.insert(*key, *value);
        }
        plain_state.storages.insert(*addr, slot_map);
    }

    plain_state
}

fn bench_state_root_overlay(c: &mut Criterion) {
    let (base_accounts, base_storage) = generate_test_data(100000, 10, SEED);

    // Create a separate group for each overlay size (generates separate violin plots)
    for overlay_size in [1000, 10000] {
        let mut group = c.benchmark_group(format!("overlay_{}", overlay_size));
        group.sample_size(10);

        let provider_factory = setup_database(&base_accounts, &base_storage);

        // Generate overlay
        let (overlay_accounts, overlay_storage) =
            generate_overlay(&base_accounts, overlay_size, SEED + 1);

        // Benchmark MDBX
        let hashed_state = to_hashed_post_state(&overlay_accounts, &overlay_storage);
        group.bench_function("mdbx", |b| {
            b.iter(|| {
                let provider = provider_factory.database_provider_ro().unwrap();
                let latest = reth_provider::LatestStateProvider::new(provider);
                latest.state_root_with_updates(hashed_state.clone()).unwrap()
            })
        });

        // Benchmark TrieDB
        #[cfg(feature = "triedb")]
        {
            let plain_state = to_plain_post_state(&overlay_accounts, &overlay_storage);
            group.bench_function("triedb", |b| {
                b.iter(|| {
                    let provider = provider_factory.database_provider_ro().unwrap();
                    let triedb = provider.triedb_provider().clone();
                    let latest =
                        reth_provider::LatestStateProvider::new_with_triedb(provider, triedb);
                    latest.state_root_with_updates_plain(plain_state.clone()).unwrap()
                })
            });
        }

        group.finish();
    }
}

criterion_group!(benches, bench_state_root_overlay);
criterion_main!(benches);
