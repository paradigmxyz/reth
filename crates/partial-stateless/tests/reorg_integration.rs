//! Integration tests for reorg handling.
//!
//! These replay the exact cache-mutation sequence the ExEx handlers perform —
//! `ChainCommitted` (apply low→high), `ChainReorged` (roll back old newest→oldest,
//! then apply new low→high), `ChainReverted` (roll back old newest→oldest) — and
//! assert the cache reflects only the canonical chain afterwards.
//!
//! The handlers' EVM simulation is out of scope here; this exercises the cache
//! contract those handlers depend on via its public API.

use alloy_primitives::{Address, B256, U256};
use partial_stateless::{
    accessed_state::BlockAccessedState,
    network_cache::NetworkStateCache,
    policy::{AccountData, LastNBlocksPolicy},
};
use std::collections::BTreeMap;

fn make_cache(account_window: u64, storage_window: u64) -> NetworkStateCache {
    NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(account_window)),
        Box::new(LastNBlocksPolicy::new(storage_window)),
    )
}

fn account(nonce: u64, balance: u64) -> AccountData {
    AccountData { nonce, balance: U256::from(balance), code_hash: None }
}

/// Build one block's accessed state from `(address, account)` and `(address, slot, value)` lists.
fn block_state(
    accounts: &[(Address, AccountData)],
    storage: &[(Address, B256, u64)],
) -> BlockAccessedState {
    let mut s = BlockAccessedState::default();
    for (addr, data) in accounts {
        s.accounts.insert(*addr, data.clone());
    }
    for (addr, slot, value) in storage {
        s.storage.insert((*addr, *slot), U256::from(*value));
    }
    s
}

/// Mirrors the `ChainCommitted` handler: apply blocks low→high.
fn commit(cache: &mut NetworkStateCache, blocks: &[(u64, BlockAccessedState)]) {
    for (number, accessed) in blocks {
        cache.on_block_executed(*number, accessed);
    }
}

/// Mirrors the `ChainReorged` handler: roll back `old` newest→oldest, cold-reset on
/// failure, then apply `new` oldest→newest. Returns whether rollback succeeded.
fn reorg(cache: &mut NetworkStateCache, old: &[u64], new: &[(u64, BlockAccessedState)]) -> bool {
    let mut rollback_ok = true;
    for number in old.iter().rev() {
        if cache.rollback_block(*number).is_err() {
            rollback_ok = false;
            break;
        }
    }
    if !rollback_ok {
        cache.reset();
    }
    for (number, accessed) in new {
        cache.on_block_executed(*number, accessed);
    }
    rollback_ok
}

/// Mirrors the `ChainReverted` handler: roll back `old` newest→oldest.
fn revert(cache: &mut NetworkStateCache, old: &[u64]) {
    for number in old.iter().rev() {
        if cache.rollback_block(*number).is_err() {
            cache.reset();
            break;
        }
    }
}

/// Full-content fingerprint (keys + values + freshness + height) for equivalence checks.
type Fingerprint =
    (BTreeMap<Address, (u64, U256, u64)>, BTreeMap<(Address, B256), (U256, u64)>, u64);

fn fingerprint(cache: &NetworkStateCache) -> Fingerprint {
    let accounts = cache
        .accounts()
        .iter()
        .map(|(k, e)| (*k, (e.value.nonce, e.value.balance, e.last_accessed_block)))
        .collect();
    let storage = cache
        .storage()
        .iter()
        .map(|(k, e)| (*k, (e.value, e.last_accessed_block)))
        .collect();
    (accounts, storage, cache.current_block())
}

#[test]
fn test_reorg_drops_old_branch_state() {
    let mut cache = make_cache(20, 20);
    let shared = Address::repeat_byte(0x01);
    let only_old = Address::repeat_byte(0x0A);
    let only_new = Address::repeat_byte(0x0B);

    commit(
        &mut cache,
        &[
            (100, block_state(&[(shared, account(1, 100))], &[])),
            (101, block_state(&[(shared, account(2, 200))], &[])),
            (102, block_state(&[(only_old, account(1, 1))], &[])), // old-branch tip
        ],
    );
    assert!(cache.contains_account(&only_old));

    // Reorg: revert old block 102, apply new block 102' touching a different account.
    reorg(&mut cache, &[102], &[(102, block_state(&[(only_new, account(1, 1))], &[]))]);

    assert!(!cache.contains_account(&only_old), "old-branch state must be gone after reorg");
    assert!(cache.contains_account(&only_new), "new-branch state must be present");
    assert!(cache.contains_account(&shared), "shared state below the fork must remain");
    assert_eq!(cache.current_block(), 102);
}

#[test]
fn test_reorg_equals_fresh_replay_invariant() {
    // The core safety invariant: applying a reorg (roll back old + apply new) must
    // yield a cache identical to applying the new canonical chain fresh from the
    // fork point. If it doesn't, sidecar hit/miss accounting diverges between peers.
    let s100 = block_state(
        &[(Address::repeat_byte(1), account(1, 100))],
        &[(Address::repeat_byte(1), B256::repeat_byte(9), 1)],
    );
    let s101 = block_state(&[(Address::repeat_byte(2), account(1, 101))], &[]);
    let old102 = block_state(
        &[(Address::repeat_byte(0xA0), account(1, 1))],
        &[(Address::repeat_byte(0xA0), B256::repeat_byte(0xAA), 5)],
    );
    let old103 = block_state(&[(Address::repeat_byte(0xA1), account(1, 2))], &[]);
    let new102 = block_state(
        &[(Address::repeat_byte(0xB0), account(7, 7))],
        &[(Address::repeat_byte(0xB0), B256::repeat_byte(0xBB), 8)],
    );
    let new103 = block_state(&[(Address::repeat_byte(0xB1), account(9, 9))], &[]);

    // Cache A: commit shared blocks + old branch, then reorg onto the new branch.
    let mut a = make_cache(50, 50);
    commit(&mut a, &[(100, s100.clone()), (101, s101.clone())]);
    commit(&mut a, &[(102, old102), (103, old103)]);
    let ok = reorg(&mut a, &[102, 103], &[(102, new102.clone()), (103, new103.clone())]);
    assert!(ok, "rollback should succeed with full undo history");

    // Cache B: commit shared blocks, then the new branch directly.
    let mut b = make_cache(50, 50);
    commit(&mut b, &[(100, s100), (101, s101), (102, new102), (103, new103)]);

    assert_eq!(fingerprint(&a), fingerprint(&b), "reorg result must equal a fresh replay");
}

#[test]
fn test_pure_revert_returns_to_prior_state() {
    let s100 = block_state(&[(Address::repeat_byte(1), account(1, 100))], &[]);
    let s101 = block_state(&[(Address::repeat_byte(2), account(1, 101))], &[]);
    let s102 = block_state(&[(Address::repeat_byte(3), account(1, 102))], &[]);

    let mut cache = make_cache(50, 50);
    commit(&mut cache, &[(100, s100.clone()), (101, s101.clone()), (102, s102)]);

    // Revert blocks 101 and 102 with no replacement.
    revert(&mut cache, &[101, 102]);

    // Reference: only block 100 was ever applied.
    let mut reference = make_cache(50, 50);
    commit(&mut reference, &[(100, s100)]);

    assert_eq!(fingerprint(&cache), fingerprint(&reference));
}

#[test]
fn test_deep_reorg_beyond_history_cold_resets() {
    let mut cache = make_cache(50, 50);
    commit(
        &mut cache,
        &[
            (100, block_state(&[(Address::repeat_byte(1), account(1, 100))], &[])),
            (101, block_state(&[(Address::repeat_byte(2), account(1, 101))], &[])),
            (102, block_state(&[(Address::repeat_byte(3), account(1, 102))], &[])),
        ],
    );

    // Finalize through 102: undo history is pruned, so block 102 can no longer roll back.
    cache.prune_undo_below(102);

    let new_addr = Address::repeat_byte(0xC0);
    let ok = reorg(&mut cache, &[102], &[(103, block_state(&[(new_addr, account(1, 1))], &[]))]);

    assert!(!ok, "rollback must fail once undo history is pruned");
    // After cold reset + reapply, only the new block's state remains.
    assert!(cache.contains_account(&new_addr));
    assert!(!cache.contains_account(&Address::repeat_byte(1)), "cold reset must clear old state");
    assert_eq!(cache.current_block(), 103);
}
