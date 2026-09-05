//! Collision eviction and state reuse through the actual fixed caches.

use super::*;
use crate::PayloadExecutionCache;
use alloy_primitives::{keccak256, U256};
use reth_revm::{
    db::{states::StorageSlot, AccountStatus},
    revm::{bytecode::Bytecode as RevmBytecode, state::AccountInfo},
};
use std::hash::{BuildHasher, Hash};

/// Pigeonhole a collision in the real cache's minimum capacity without weakening its hash.
fn collision<K: Copy + Ord + Hash>(
    hasher: &impl BuildHasher,
    capacity: usize,
    key: impl Fn(usize) -> K,
) -> [K; 2] {
    let mut buckets = vec![None; capacity];
    for n in 0..=capacity {
        let key = key(n);
        let hash = hasher.hash_one(key);
        let hash = if cfg!(target_pointer_width = "32") {
            (hash >> 32) as usize ^ hash as usize
        } else {
            hash as usize
        };
        let bucket = &mut buckets[hash & (capacity - 1)];
        if let Some(previous) = bucket.replace(key) {
            let mut pair = [previous, key];
            pair.sort_unstable();
            return pair
        }
    }
    unreachable!("capacity + 1 distinct keys must collide")
}

#[derive(Debug)]
struct Fixture {
    accounts: [Address; 2],
    storage: [U256; 2],
    contracts: [(B256, RevmBytecode); 2],
}

impl Fixture {
    fn new() -> Self {
        let cache = ExecutionCache::new_deterministic(1);
        let accounts =
            collision(cache.0.account_cache.hasher(), cache.0.account_cache.capacity(), |n| {
                Address::from_slice(&keccak256(n.to_be_bytes())[..20])
            });
        let slots =
            collision(cache.0.storage_cache.hasher(), cache.0.storage_cache.capacity(), |n| {
                (accounts[0], B256::from(U256::from(n)))
            });
        let code_for = |n: usize| RevmBytecode::new_raw(n.to_be_bytes().to_vec().into());
        let hashes = collision(cache.0.code_cache.hasher(), cache.0.code_cache.capacity(), |n| {
            code_for(n).hash_slow()
        });
        let contracts = hashes.map(|hash| {
            let code = (0..=cache.0.code_cache.capacity())
                .map(code_for)
                .find(|code| code.hash_slow() == hash)
                .unwrap();
            (hash, code)
        });
        Self { accounts, storage: slots.map(|(_, key)| U256::from_be_bytes(key.0)), contracts }
    }

    fn bundle(&self, reverse: bool) -> BundleState {
        let mut bundle = BundleState::default();
        let order = if reverse { [1, 0] } else { [0, 1] };
        for i in order {
            let info = AccountInfo { nonce: i as u64 + 1, ..Default::default() };
            let mut account =
                BundleAccount::new(None, Some(info), Default::default(), AccountStatus::Changed);
            if i == 0 {
                for j in order {
                    account.storage.insert(
                        self.storage[j],
                        StorageSlot::new_changed(U256::ZERO, U256::from(j + 1)),
                    );
                }
            }
            bundle.state.insert(self.accounts[i], account);
            let (hash, code) = &self.contracts[i];
            bundle.contracts.insert(*hash, code.clone());
        }
        bundle
    }

    fn assert_retained_entries(&self, cache: &ExecutionCache) {
        assert_eq!(cache.0.account_stats.collisions(), 1);
        assert_eq!(cache.0.storage_stats.collisions(), 1);
        assert_eq!(cache.0.code_stats.collisions(), 1);
        assert_eq!(cache.0.account_cache.get(&self.accounts[0]), None);
        assert_eq!(cache.0.account_cache.get(&self.accounts[1]).flatten().unwrap().nonce, 2);
        assert_eq!(cache.0.storage_cache.get(&(self.accounts[0], self.storage[0].into())), None);
        assert_eq!(
            cache.0.storage_cache.get(&(self.accounts[0], self.storage[1].into())),
            Some(U256::from(2))
        );
        assert_eq!(cache.0.code_cache.get(&self.contracts[0].0), None);
        assert_eq!(
            cache.0.code_cache.get(&self.contracts[1].0),
            Some(Some(Bytecode(self.contracts[1].1.clone())))
        );
    }

    fn assert_lookups(&self, cache: &ExecutionCache) {
        for i in 0..2 {
            let account = Some(Account { nonce: i as u64 + 1, ..Default::default() });
            let code = Some(Bytecode(self.contracts[i].1.clone()));
            let storage = U256::from(i + 1);
            assert_eq!(
                value(
                    cache.get_or_try_insert_account_with(self.accounts[i], || Ok::<_, ()>(account))
                ),
                account
            );
            assert_eq!(
                value(cache.get_or_try_insert_storage_with(
                    self.accounts[0],
                    self.storage[i].into(),
                    || Ok::<_, ()>(storage)
                )),
                storage
            );
            assert_eq!(
                value(cache.get_or_try_insert_code_with(self.contracts[i].0, || Ok::<_, ()>(
                    code.clone()
                ))),
                code
            );
        }
    }
}

fn value<T>(result: Result<CachedStatus<T>, ()>) -> T {
    match result.unwrap() {
        CachedStatus::Cached(value) | CachedStatus::NotCached(value) => value,
    }
}

#[test]
fn deterministic_collision_eviction_and_native_lookup_parity() {
    let fixture = Fixture::new();
    // Rebuild independently randomized BundleState maps, with opposing insertion orders.
    for reverse in [false, true, false, true] {
        let bundle = fixture.bundle(reverse);
        let deterministic = ExecutionCache::new_deterministic(1);
        deterministic.insert_state(&bundle).unwrap();
        fixture.assert_retained_entries(&deterministic);
        fixture.assert_lookups(&deterministic);

        let native = ExecutionCache::new(1);
        native.insert_state(&bundle).unwrap();
        // Residency may differ with native seeds; every hit and fallback must return the same
        // state.
        fixture.assert_lookups(&native);
    }
}

#[test]
fn deterministic_cache_reuse_reorg_and_invalid_state() {
    let shared = PayloadExecutionCache::default();
    let parent = B256::repeat_byte(1);
    let fork = B256::repeat_byte(2);
    let address = Address::repeat_byte(3);
    let key = B256::repeat_byte(4);
    let code = RevmBytecode::new_raw([0x60, 0x01].into());
    let code_hash = code.hash_slow();
    shared.update_with_guard(|slot| {
        let cache = ExecutionCache::new_deterministic(1);
        cache.insert_account(address, Some(Account { nonce: 7, ..Default::default() }));
        cache.insert_storage(address, key, Some(U256::from(9)));
        cache.insert_code(code_hash, Some(Bytecode(code.clone())));
        *slot = Some(SavedCache::new(parent, cache));
    });

    let saved = shared.get_cache_for(parent).unwrap();
    let worker_cache = saved.cache().clone();
    drop(saved);
    assert!(shared.get_cache_for(parent).is_none());
    assert!(shared.get_cache_for(fork).is_none(), "a live worker prevents reorg invalidation");
    drop(worker_cache);

    let saved = shared.get_cache_for(parent).unwrap();
    assert_eq!(saved.cache().0.account_cache.get(&address).flatten().unwrap().nonce, 7);
    drop(saved);
    let saved = shared.get_cache_for(fork).unwrap();
    assert_eq!(saved.executed_block_hash(), fork);
    assert!(saved.cache().0.account_cache.get(&address).is_none());
    assert!(saved.cache().0.storage_cache.get(&(address, key)).is_none());
    assert_eq!(saved.cache().0.code_cache.get(&code_hash), Some(Some(Bytecode(code))));

    let mut invalid = BundleState::default();
    invalid.state.insert(
        address,
        BundleAccount::new(None, None, Default::default(), AccountStatus::Changed),
    );
    assert!(
        saved.cache().insert_state(&invalid).is_err(),
        "invalid state must reach the caller's discard path"
    );
}

#[test]
fn deterministic_selfdestruct_invalidates_state_but_keeps_code() {
    let fixture = Fixture::new();
    for make_cache in [ExecutionCache::new, ExecutionCache::new_deterministic] {
        let cache = make_cache(1);
        let mut bundle = fixture.bundle(false);
        cache.insert_state(&bundle).unwrap();
        let destroyed = bundle.state.get_mut(&fixture.accounts[1]).unwrap();
        destroyed.original_info =
            Some(AccountInfo { code_hash: fixture.contracts[0].0, ..Default::default() });
        destroyed.info = None;
        destroyed.status = AccountStatus::Destroyed;
        cache.insert_state(&bundle).unwrap();
        for address in fixture.accounts {
            assert!(cache.0.account_cache.get(&address).is_none());
            for key in fixture.storage {
                assert!(cache.0.storage_cache.get(&(address, key.into())).is_none());
            }
        }
        // Bytecode hashes identify immutable data even when state is invalidated.
        for (hash, code) in &fixture.contracts {
            assert_eq!(
                value(cache.get_or_try_insert_code_with(*hash, || Ok::<_, ()>(Some(Bytecode(
                    code.clone()
                ))))),
                Some(Bytecode(code.clone()))
            );
        }
    }
}
