#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, U256,
};
use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::mock::MockHashedCursorFactory,
    proof::Proof,
    trie_cursor::mock::MockTrieCursorFactory,
    updates::{StorageTrieUpdates, TrieUpdates},
    StateRoot,
};
use reth_trie_common::{HashedPostState, MultiProofTargets};

fn b256_from_u64(value: u64) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    B256::from(bytes)
}

fn make_targets(accounts: usize, slots_per_account: usize, start: u64) -> MultiProofTargets {
    let mut targets = MultiProofTargets::with_capacity(accounts);
    for i in 0..accounts {
        let address = b256_from_u64(start + i as u64);
        let mut slots = B256Set::default();
        for j in 0..slots_per_account {
            let slot_key = (start as u128) << 32 | (i as u128) << 16 | j as u128;
            slots.insert(b256_from_u64(slot_key as u64));
        }
        targets.insert(address, slots);
    }
    targets
}

fn make_hashed_post_state(accounts: usize) -> (HashedPostState, Vec<B256>) {
    let mut account_map = B256Map::with_capacity_and_hasher(accounts, Default::default());
    let mut addresses = Vec::with_capacity(accounts);

    for i in 0..accounts {
        let address = b256_from_u64(i as u64 + 1);
        addresses.push(address);
        account_map.insert(
            address,
            Some(Account { nonce: i as u64, balance: U256::from(i as u64), bytecode_hash: None }),
        );
    }

    let post_state = HashedPostState { accounts: account_map, storages: B256Map::default() };
    (post_state, addresses)
}

fn create_state_trie_factories(
    post_state: &HashedPostState,
) -> (MockTrieCursorFactory, MockHashedCursorFactory) {
    let mut storage_tries: B256Map<StorageTrieUpdates> = B256Map::default();
    for address in post_state.accounts.keys().chain(post_state.storages.keys()) {
        storage_tries.entry(*address).or_default();
    }

    let empty_trie_cursor_factory = MockTrieCursorFactory::from_trie_updates(TrieUpdates {
        storage_tries: storage_tries.clone(),
        ..Default::default()
    });

    let hashed_cursor_factory = MockHashedCursorFactory::from_hashed_post_state(post_state.clone());

    let (_root, mut trie_updates) =
        StateRoot::new(empty_trie_cursor_factory, hashed_cursor_factory.clone())
            .root_with_updates()
            .expect("StateRoot should succeed");

    trie_updates.storage_tries = storage_tries;

    let trie_cursor_factory = MockTrieCursorFactory::from_trie_updates(trie_updates);
    (trie_cursor_factory, hashed_cursor_factory)
}

fn bench_multiproof_targets_extend(c: &mut Criterion) {
    let mut group = c.benchmark_group("MultiProofTargets::extend");
    for (accounts, slots_per_account) in [(256, 4), (1024, 2), (2048, 1)] {
        let id = format!("accounts_{accounts}/slots_{slots_per_account}");
        group.bench_function(BenchmarkId::new("extend_owned", &id), |b| {
            b.iter_batched(
                || {
                    let acc = make_targets(accounts, slots_per_account, 0);
                    let other = make_targets(accounts, slots_per_account, 10_000);
                    (acc, other)
                },
                |(mut acc, other)| {
                    acc.extend(other);
                    black_box(acc);
                },
                BatchSize::SmallInput,
            );
        });
    }
}

fn bench_proof_multiproof_allocs(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proof::multiproof");
    for accounts in [128usize, 512, 2048] {
        let (post_state, addresses) = make_hashed_post_state(accounts);
        let (trie_cursor_factory, hashed_cursor_factory) = create_state_trie_factories(&post_state);
        let bench_id = format!("accounts_{accounts}");

        group.bench_function(BenchmarkId::new("account_only", &bench_id), |b| {
            b.iter_batched(
                || MultiProofTargets::accounts(addresses.iter().copied()),
                |targets| {
                    let proof = Proof::new(
                        trie_cursor_factory.clone(),
                        hashed_cursor_factory.clone(),
                    )
                    .multiproof(targets)
                    .expect("multiproof should succeed");
                    black_box(proof);
                },
                BatchSize::SmallInput,
            );
        });
    }
}

criterion_group!(multiproof_allocs, bench_multiproof_targets_extend, bench_proof_multiproof_allocs);
criterion_main!(multiproof_allocs);
