//! End-to-end tests for warming a cold node from an untrusted peer snapshot.

use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use std::collections::HashMap;

use partial_stateless::{
    bootstrap::{verify_and_restore, BootstrapError, CacheSnapshotPackage},
    network_cache::{CachedEntry, NetworkStateCache},
    policy::{AccountData, LastNBlocksPolicy},
    sidecar::{last_n_blocks_cache_policy_id, CacheAnchor},
};

const ANCHOR_BLOCK: u64 = 100;
const ACCOUNT_WINDOW: u64 = 64;
const STORAGE_CODE_WINDOW: u64 = 32;

fn entry<T>(value: T) -> CachedEntry<T> {
    CachedEntry { value, first_accessed_block: 90, last_accessed_block: 98, access_count: 3 }
}

fn policies() -> (Box<LastNBlocksPolicy>, Box<LastNBlocksPolicy>) {
    (
        Box::new(LastNBlocksPolicy::new(ACCOUNT_WINDOW)),
        Box::new(LastNBlocksPolicy::new(STORAGE_CODE_WINDOW)),
    )
}

/// A warm cache with one account, one storage slot, and one bytecode whose key
/// is the true keccak256 of its bytes, restored at `ANCHOR_BLOCK`.
fn warm_cache() -> NetworkStateCache {
    let code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xf3]);
    let code_hash = keccak256(&code);

    let mut accounts = HashMap::new();
    accounts.insert(
        Address::repeat_byte(0x11),
        entry(AccountData { nonce: 7, balance: U256::from(1_000u64), code_hash: Some(code_hash) }),
    );

    let mut storage = HashMap::new();
    storage.insert((Address::repeat_byte(0x11), B256::repeat_byte(0x01)), entry(U256::from(42u64)));

    let mut codes = HashMap::new();
    codes.insert(code_hash, entry(code));

    let (account_policy, storage_policy) = policies();
    NetworkStateCache::restore(
        accounts,
        storage,
        codes,
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    )
}

fn anchor_for(cache: &NetworkStateCache) -> CacheAnchor {
    let policy_id = last_n_blocks_cache_policy_id(ACCOUNT_WINDOW, STORAGE_CODE_WINDOW);
    cache.cache_anchor(ANCHOR_BLOCK, B256::repeat_byte(0xAB), policy_id)
}

/// `NetworkStateCache` is not `Debug`, so `Result::unwrap_err` is unavailable.
fn expect_reject(result: Result<NetworkStateCache, BootstrapError>) -> BootstrapError {
    match result {
        Ok(_) => panic!("expected snapshot to be rejected"),
        Err(e) => e,
    }
}

#[test]
fn roundtrip_restores_identical_cache() {
    let cache = warm_cache();
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor);

    let (account_policy, storage_policy) = policies();
    let restored = verify_and_restore(pkg, &anchor, account_policy, storage_policy)
        .expect("honest snapshot must verify");

    assert_eq!(restored.cache_root(), cache.cache_root());
    assert_eq!(restored.current_block(), ANCHOR_BLOCK);
}

#[test]
fn rejects_tampered_account_value() {
    let cache = warm_cache();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor);

    // Flip a balance without touching the committed anchor: cache_root diverges.
    pkg.state.accounts[0].1.value.balance = U256::from(999_999u64);

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(pkg, &anchor, account_policy, storage_policy));
    assert!(matches!(err, BootstrapError::CacheRootMismatch { .. }), "got {err:?}");
}

#[test]
fn rejects_anchor_from_different_policy() {
    let cache = warm_cache();
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor_for(&cache));

    // Consensus expects a different policy window than the peer's snapshot.
    let wrong_policy_id = last_n_blocks_cache_policy_id(ACCOUNT_WINDOW + 1, STORAGE_CODE_WINDOW);
    let expected = cache.cache_anchor(ANCHOR_BLOCK, B256::repeat_byte(0xAB), wrong_policy_id);

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(pkg, &expected, account_policy, storage_policy));
    assert!(matches!(err, BootstrapError::AnchorMismatch { .. }), "got {err:?}");
}

#[test]
fn rejects_state_block_skew() {
    let cache = warm_cache();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor);

    // current_block is not covered by cache_root, so it must be checked directly.
    pkg.state.current_block = ANCHOR_BLOCK - 1;

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(pkg, &anchor, account_policy, storage_policy));
    assert!(
        matches!(err, BootstrapError::StateBlockMismatch { state_block, anchor_block }
            if state_block == ANCHOR_BLOCK - 1 && anchor_block == ANCHOR_BLOCK),
        "got {err:?}"
    );
}

#[test]
fn rejects_forged_bytecode_under_valid_hash() {
    let cache = warm_cache();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor);

    // Keep the code_hash key but ship different bytes. The preimage check runs
    // before the cache-root check so this reports the more specific error.
    pkg.state.codes[0].1.value = Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]);

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(pkg, &anchor, account_policy, storage_policy));
    assert!(matches!(err, BootstrapError::BytecodePreimageMismatch { .. }), "got {err:?}");
}
