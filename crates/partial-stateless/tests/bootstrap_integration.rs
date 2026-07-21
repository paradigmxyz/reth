//! End-to-end tests for restoring coordinated value and sparse-trie caches.

use alloy_primitives::{keccak256, map::B256Map, Address, Bytes, B256, U256};
use partial_stateless::{
    bootstrap::{
        build_snapshot_package, verify_and_restore, verify_and_restore_with_limits, BootstrapError,
        BootstrapLimits, CacheSnapshotPackage, RestoredBootstrapState,
    },
    network_cache::{CachedEntry, NetworkStateCache},
    policy::{AccountData, LastNBlocksPolicy},
    sidecar::{last_n_blocks_cache_policy_id, CacheAnchor},
    try_compute_trustless_state_root, BlockAccessedState, CacheState, PartialTrieNodeCache,
};
use reth_primitives_traits::Account;
use reth_trie::HashBuilder;
use reth_trie_common::{
    proof::ProofRetainer, MultiProof, Nibbles, StorageMultiProof, EMPTY_ROOT_HASH,
};
use revm_database::{AccountStatus, BundleAccount, BundleState};
use revm_state::AccountInfo;
use std::collections::HashMap;

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

fn clone_cache(cache: &NetworkStateCache) -> NetworkStateCache {
    let (account_policy, storage_policy) = policies();
    CacheState::from_cache(cache).into_cache(account_policy, storage_policy)
}

fn anchor_for(cache: &NetworkStateCache) -> CacheAnchor {
    let policy_id = last_n_blocks_cache_policy_id(ACCOUNT_WINDOW, STORAGE_CODE_WINDOW);
    cache.cache_anchor(ANCHOR_BLOCK, B256::repeat_byte(0xAB), policy_id)
}

fn state_proof(
    address: Address,
    account: Account,
    state_storage: &[(B256, U256)],
    target_slots: &[B256],
) -> (MultiProof, B256) {
    let hashed_address = keccak256(address);
    let target_paths = target_slots.iter().map(|slot| Nibbles::unpack(keccak256(slot)));
    let mut storage_builder =
        HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter(target_paths));
    let mut storage_leaves = state_storage
        .iter()
        .filter(|(_, value)| !value.is_zero())
        .map(|(slot, value)| (Nibbles::unpack(keccak256(slot)), alloy_rlp::encode(value)))
        .collect::<Vec<_>>();
    storage_leaves.sort_by_key(|(path, _)| *path);
    for (path, value) in storage_leaves {
        storage_builder.add_leaf(path, &value);
    }
    let storage_root = storage_builder.root();
    let storage_subtree = storage_builder.take_proof_nodes();

    let address_path = Nibbles::unpack(hashed_address);
    let mut account_builder =
        HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([address_path]));
    account_builder
        .add_leaf(address_path, &alloy_rlp::encode(account.into_trie_account(storage_root)));
    let state_root = account_builder.root();
    let account_subtree = account_builder.take_proof_nodes();

    let mut storages = B256Map::default();
    if !target_slots.is_empty() {
        storages.insert(
            hashed_address,
            StorageMultiProof {
                root: storage_root,
                subtree: storage_subtree,
                branch_node_masks: Default::default(),
            },
        );
    }

    (MultiProof { account_subtree, branch_node_masks: Default::default(), storages }, state_root)
}

fn missing_account_proof(
    existing_address: Address,
    existing_account: Account,
    missing_address: Address,
) -> (MultiProof, B256) {
    let missing_path = Nibbles::unpack(keccak256(missing_address));
    let existing_path = Nibbles::unpack(keccak256(existing_address));
    let mut builder =
        HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([missing_path]));
    builder.add_leaf(
        existing_path,
        &alloy_rlp::encode(existing_account.into_trie_account(EMPTY_ROOT_HASH)),
    );
    let state_root = builder.root();
    (
        MultiProof {
            account_subtree: builder.take_proof_nodes(),
            branch_node_masks: Default::default(),
            storages: Default::default(),
        },
        state_root,
    )
}

fn warm_fixture() -> (NetworkStateCache, MultiProof, B256, Address, B256) {
    let address = Address::repeat_byte(0x11);
    let slot = B256::repeat_byte(0x01);
    let code = Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xf3]);
    let code_hash = keccak256(&code);
    let account =
        Account { nonce: 7, balance: U256::from(1_000u64), bytecode_hash: Some(code_hash) };
    let (proof, state_root) = state_proof(address, account, &[(slot, U256::from(42))], &[slot]);

    let mut accounts = HashMap::new();
    accounts.insert(
        address,
        entry(AccountData {
            nonce: account.nonce,
            balance: account.balance,
            code_hash: account.bytecode_hash,
        }),
    );
    let mut storage = HashMap::new();
    storage.insert((address, slot), entry(U256::from(42u64)));
    let mut codes = HashMap::new();
    codes.insert(code_hash, entry(code));

    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        accounts,
        storage,
        codes,
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    (cache, proof, state_root, address, slot)
}

fn expect_reject(result: Result<RestoredBootstrapState, BootstrapError>) -> BootstrapError {
    match result {
        Ok(_) => panic!("expected snapshot to be rejected"),
        Err(err) => err,
    }
}

#[test]
fn roundtrip_restores_both_coordinated_caches() {
    let (cache, proof, state_root, address, slot) = warm_fixture();
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);

    let (account_policy, storage_policy) = policies();
    let mut restored = verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy)
        .expect("honest snapshot must verify");

    assert_eq!(restored.value_cache.cache_root(), cache.cache_root());
    assert_eq!(restored.value_cache.current_block(), ANCHOR_BLOCK);
    assert_eq!(restored.trie_cache.state_root(), Some(state_root));
    assert!(restored.trie_cache.contains_account_path(&address));
    assert!(restored.trie_cache.contains_storage_path(&address, &slot));
    restored.trie_cache.validate_against_value_cache(&restored.value_cache).unwrap();
}

#[test]
fn package_builder_requests_all_cached_paths() {
    let (cache, proof, state_root, address, slot) = warm_fixture();
    let anchor = anchor_for(&cache);
    let mut requested = None;
    let pkg = build_snapshot_package(&cache, anchor, state_root, |targets| {
        requested = Some(targets);
        Ok(proof.clone())
    })
    .expect("provider proof should build a package");

    let requested = requested.expect("proof provider must be called");
    assert!(requested.contains_key(&keccak256(address)));
    assert!(requested[&keccak256(address)].contains(&keccak256(slot)));
    assert_eq!(pkg.anchor, anchor);
}

#[test]
fn restores_cached_nonexistent_account_exclusion_path() {
    let missing = Address::repeat_byte(0x33);
    let existing = Address::repeat_byte(0x32);
    let existing_account = Account { nonce: 1, balance: U256::from(10), bytecode_hash: None };
    let (proof, state_root) = missing_account_proof(existing, existing_account, missing);

    let mut accounts = HashMap::new();
    accounts.insert(missing, entry(AccountData { nonce: 0, balance: U256::ZERO, code_hash: None }));
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        accounts,
        HashMap::new(),
        HashMap::new(),
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let (account_policy, storage_policy) = policies();
    let mut restored =
        verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy).unwrap();

    assert!(restored.trie_cache.contains_account_path(&missing));
    restored.trie_cache.validate_against_value_cache(&restored.value_cache).unwrap();
}

#[test]
fn restores_zero_storage_exclusion_path() {
    let address = Address::repeat_byte(0x41);
    let target_slot = B256::repeat_byte(0x42);
    let existing_slot = B256::repeat_byte(0x43);
    let account = Account { nonce: 1, balance: U256::from(20), bytecode_hash: None };
    let (proof, state_root) =
        state_proof(address, account, &[(existing_slot, U256::from(9))], &[target_slot]);

    let mut accounts = HashMap::new();
    accounts.insert(
        address,
        entry(AccountData {
            nonce: account.nonce,
            balance: account.balance,
            code_hash: account.bytecode_hash,
        }),
    );
    let mut storage = HashMap::new();
    storage.insert((address, target_slot), entry(U256::ZERO));
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        accounts,
        storage,
        HashMap::new(),
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let (account_policy, storage_policy) = policies();
    let restored =
        verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy).unwrap();

    assert!(restored.trie_cache.contains_storage_path(&address, &target_slot));
}

#[test]
fn restores_account_path_for_storage_only_entry() {
    let address = Address::repeat_byte(0x51);
    let slot = B256::repeat_byte(0x52);
    let value = U256::from(77);
    let account = Account { nonce: 3, balance: U256::from(30), bytecode_hash: None };
    let (proof, state_root) = state_proof(address, account, &[(slot, value)], &[slot]);

    let mut storage = HashMap::new();
    storage.insert((address, slot), entry(value));
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        HashMap::new(),
        storage,
        HashMap::new(),
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let (account_policy, storage_policy) = policies();
    let mut restored =
        verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy).unwrap();

    assert_eq!(restored.trie_cache.tracked_account_count(), 0);
    assert_eq!(restored.trie_cache.tracked_storage_slot_count(), 1);
    assert!(restored.trie_cache.contains_account_path(&address));
    assert!(restored.trie_cache.contains_storage_path(&address, &slot));
    restored.trie_cache.validate_against_value_cache(&restored.value_cache).unwrap();
}

#[test]
fn empty_value_namespaces_restore_with_cold_trie() {
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &MultiProof::default());
    let (account_policy, storage_policy) = policies();
    let restored =
        verify_and_restore(pkg, &anchor, B256::repeat_byte(0x99), account_policy, storage_policy)
            .unwrap();

    assert_eq!(restored.trie_cache.state_root(), None);
    assert_eq!(restored.trie_cache.tracked_account_count(), 0);
    assert_eq!(restored.trie_cache.tracked_storage_slot_count(), 0);
}

#[test]
fn code_only_cache_restores_without_trie_paths() {
    let code = Bytes::from_static(&[0x60, 0x01]);
    let code_hash = keccak256(&code);
    let mut codes = HashMap::new();
    codes.insert(code_hash, entry(code));
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        HashMap::new(),
        HashMap::new(),
        codes,
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &MultiProof::default());
    let (account_policy, storage_policy) = policies();
    let restored =
        verify_and_restore(pkg, &anchor, B256::repeat_byte(0x99), account_policy, storage_policy)
            .unwrap();

    assert!(restored.value_cache.contains_code(&code_hash));
    assert_eq!(restored.trie_cache.state_root(), None);
}

#[test]
fn rejects_tampered_account_value() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    pkg.state.accounts[0].1.value.balance = U256::from(999_999u64);

    let (account_policy, storage_policy) = policies();
    let err =
        expect_reject(verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy));
    assert!(matches!(err, BootstrapError::CacheRootMismatch { .. }), "got {err:?}");
}

#[test]
fn rejects_account_value_that_disagrees_with_proof_under_matching_cache_anchor() {
    let (cache, proof, state_root, address, _) = warm_fixture();
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor_for(&cache), &proof);
    pkg.state.accounts[0].1.value.balance = U256::from(999_999u64);
    let (account_policy, storage_policy) = policies();
    let modified = pkg.state.clone().into_cache(account_policy, storage_policy);
    let modified_anchor = anchor_for(&modified);
    pkg.anchor = modified_anchor;

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(
        pkg,
        &modified_anchor,
        state_root,
        account_policy,
        storage_policy,
    ));
    assert_eq!(err, BootstrapError::AccountValueMismatch { address });
}

#[test]
fn rejects_storage_value_that_disagrees_with_proof_under_matching_cache_anchor() {
    let (cache, proof, state_root, address, slot) = warm_fixture();
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor_for(&cache), &proof);
    pkg.state.storage[0].1.value = U256::from(999_999u64);
    let (account_policy, storage_policy) = policies();
    let modified = pkg.state.clone().into_cache(account_policy, storage_policy);
    let modified_anchor = anchor_for(&modified);
    pkg.anchor = modified_anchor;

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(
        pkg,
        &modified_anchor,
        state_root,
        account_policy,
        storage_policy,
    ));
    assert_eq!(
        err,
        BootstrapError::StorageValueMismatch {
            address,
            slot,
            expected: U256::from(999_999u64),
            actual: U256::from(42u64),
        }
    );
}

#[test]
fn rejects_tampered_proof_node() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let node = &mut pkg.proof.account_subtree[0].1;
    let last = node.len() - 1;
    node[last] ^= 0xff;
    let (account_policy, storage_policy) = policies();
    let err =
        expect_reject(verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy));
    assert!(matches!(err, BootstrapError::ProofVerification(_)));
}

#[test]
fn rejects_invalid_serialized_nibble_path_without_panicking() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    pkg.proof.account_subtree[0].0 = vec![0xff];
    let (account_policy, storage_policy) = policies();
    let err =
        expect_reject(verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy));
    assert!(
        matches!(err, BootstrapError::ProofVerification(message) if message.contains("invalid nibble"))
    );
}

#[test]
fn rejects_missing_or_wrong_root_proof() {
    let (cache, _, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &MultiProof::default());
    let (account_policy, storage_policy) = policies();
    let missing =
        expect_reject(verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy));
    assert!(matches!(missing, BootstrapError::ProofVerification(_)));

    let (cache, proof, _, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let (account_policy, storage_policy) = policies();
    let wrong_root = expect_reject(verify_and_restore(
        pkg,
        &anchor,
        B256::repeat_byte(0xFE),
        account_policy,
        storage_policy,
    ));
    assert!(matches!(wrong_root, BootstrapError::ProofVerification(_)));
}

#[test]
fn rejects_anchor_from_different_policy() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor_for(&cache), &proof);
    let wrong_policy_id = last_n_blocks_cache_policy_id(ACCOUNT_WINDOW + 1, STORAGE_CODE_WINDOW);
    let expected = cache.cache_anchor(ANCHOR_BLOCK, B256::repeat_byte(0xAB), wrong_policy_id);

    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore(
        pkg,
        &expected,
        state_root,
        account_policy,
        storage_policy,
    ));
    assert!(matches!(err, BootstrapError::AnchorMismatch { .. }), "got {err:?}");
}

#[test]
fn rejects_state_block_skew() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    pkg.state.current_block = ANCHOR_BLOCK - 1;

    let (account_policy, storage_policy) = policies();
    let err =
        expect_reject(verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy));
    assert!(
        matches!(err, BootstrapError::StateBlockMismatch { state_block, anchor_block }
            if state_block == ANCHOR_BLOCK - 1 && anchor_block == ANCHOR_BLOCK),
        "got {err:?}"
    );
}

#[test]
fn rejects_forged_bytecode_under_valid_hash() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let mut pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    pkg.state.codes[0].1.value = Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]);

    let (account_policy, storage_policy) = policies();
    let err =
        expect_reject(verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy));
    assert!(matches!(err, BootstrapError::BytecodePreimageMismatch { .. }), "got {err:?}");
}

#[test]
fn applies_resource_limits_before_proof_work() {
    let (cache, proof, state_root, _, _) = warm_fixture();
    let anchor = anchor_for(&cache);
    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let limits = BootstrapLimits { max_accounts: 0, ..BootstrapLimits::default() };
    let (account_policy, storage_policy) = policies();
    let err = expect_reject(verify_and_restore_with_limits(
        pkg,
        &anchor,
        state_root,
        account_policy,
        storage_policy,
        &limits,
    ));
    assert_eq!(err, BootstrapError::LimitExceeded { label: "accounts", actual: 1, cap: 0 });
}

fn structural_retry_fixture() -> (NetworkStateCache, MultiProof, MultiProof, B256) {
    let address = Address::repeat_byte(0x71);
    let account = Account { nonce: 1, balance: U256::from(10), bytecode_hash: None };
    let slot_from_u64 = |value: u64| {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        B256::from(bytes)
    };
    let mut first_by_prefix = [None; 256];
    let mut colliding = None;
    for value in 0..10_000 {
        let slot = slot_from_u64(value);
        let path = Nibbles::unpack(keccak256(slot));
        let prefix = (path.get_unchecked(0) as usize) << 4 | path.get_unchecked(1) as usize;
        if let Some(previous) = first_by_prefix[prefix] {
            colliding = Some((path.get_unchecked(0), path.get_unchecked(1), previous, slot));
            break
        }
        first_by_prefix[prefix] = Some(slot);
    }
    let (first, second, slot_a, slot_b) = colliding.unwrap();
    let slot_c = (10_000..20_000)
        .map(slot_from_u64)
        .find(|slot| Nibbles::unpack(keccak256(slot)).get_unchecked(0) != first)
        .unwrap();
    let target_slot = (10_000..20_000)
        .map(slot_from_u64)
        .find(|slot| {
            let path = Nibbles::unpack(keccak256(slot));
            path.get_unchecked(0) == first && path.get_unchecked(1) != second
        })
        .unwrap();
    let target_path = Nibbles::unpack(keccak256(target_slot));
    let mut existing = vec![
        (Nibbles::unpack(keccak256(slot_a)), U256::from(3)),
        (Nibbles::unpack(keccak256(slot_b)), U256::from(4)),
        (Nibbles::unpack(keccak256(slot_c)), U256::from(6)),
    ];
    existing.sort_unstable_by_key(|(path, _)| *path);

    let mut base_storage_builder =
        HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([target_path]));
    for (path, value) in &existing {
        base_storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
    }
    let storage_root = base_storage_builder.root();
    let base_storage_subtree = base_storage_builder.take_proof_nodes();

    let path_a = Nibbles::unpack(keccak256(slot_a));
    let path_b = Nibbles::unpack(keccak256(slot_b));
    let mut common_len = 0;
    while common_len < path_a.len() &&
        path_a.get_unchecked(common_len) == path_b.get_unchecked(common_len)
    {
        common_len += 1;
    }
    let requested_path = path_a.slice(0..common_len);
    let mut enriched_storage_builder = HashBuilder::default()
        .with_proof_retainer(ProofRetainer::from_iter([target_path, requested_path]));
    for (path, value) in &existing {
        enriched_storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
    }
    assert_eq!(enriched_storage_builder.root(), storage_root);
    let enriched_storage_subtree = enriched_storage_builder.take_proof_nodes();

    let hashed_address = keccak256(address);
    let address_path = Nibbles::unpack(hashed_address);
    let mut account_builder =
        HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([address_path]));
    account_builder
        .add_leaf(address_path, &alloy_rlp::encode(account.into_trie_account(storage_root)));
    let state_root = account_builder.root();
    let account_subtree = account_builder.take_proof_nodes();
    let make_proof = |subtree| {
        let mut storages = B256Map::default();
        storages.insert(
            hashed_address,
            StorageMultiProof {
                root: storage_root,
                subtree,
                branch_node_masks: Default::default(),
            },
        );
        MultiProof {
            account_subtree: account_subtree.clone(),
            branch_node_masks: Default::default(),
            storages,
        }
    };
    let base_proof = make_proof(base_storage_subtree);
    let enriched_proof = make_proof(enriched_storage_subtree);

    let mut accounts = HashMap::new();
    accounts.insert(
        address,
        entry(AccountData {
            nonce: account.nonce,
            balance: account.balance,
            code_hash: account.bytecode_hash,
        }),
    );
    let mut storage = HashMap::new();
    storage.insert((address, target_slot), entry(U256::ZERO));
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        accounts,
        storage,
        HashMap::new(),
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    (cache, base_proof, enriched_proof, state_root)
}

#[test]
fn package_builder_retries_structural_targets_and_detects_no_progress() {
    let (cache, base_proof, enriched_proof, state_root) = structural_retry_fixture();
    let anchor = anchor_for(&cache);
    let mut calls = 0;
    let pkg = build_snapshot_package(&cache, anchor, state_root, |_| {
        calls += 1;
        Ok(if calls == 1 { base_proof.clone() } else { enriched_proof.clone() })
    })
    .expect("builder should retry with the requested extension child");
    assert_eq!(calls, 2);
    assert_eq!(
        pkg.proof,
        partial_stateless::SerializableMultiProof::from_multiproof(&enriched_proof)
    );

    let mut calls = 0;
    let err = build_snapshot_package(&cache, anchor, state_root, |_| {
        calls += 1;
        Ok(base_proof.clone())
    })
    .unwrap_err();
    assert_eq!(calls, 2);
    assert_eq!(err, BootstrapError::ProofRetryNoProgress { retries: 2 });
}

fn account_info(account: Account) -> AccountInfo {
    AccountInfo {
        balance: account.balance,
        nonce: account.nonce,
        code_hash: account.bytecode_hash.unwrap_or_default(),
        code: None,
        ..Default::default()
    }
}

#[test]
fn bootstrapped_pair_advances_identically_to_continuous_pair() {
    let address = Address::repeat_byte(0x61);
    let code_hash = B256::repeat_byte(0x62);
    let parent = Account { nonce: 1, balance: U256::from(10), bytecode_hash: Some(code_hash) };
    let updated = Account { nonce: 2, balance: U256::from(20), bytecode_hash: Some(code_hash) };
    let (proof, state_root) = state_proof(address, parent, &[], &[]);

    let mut accounts = HashMap::new();
    accounts.insert(
        address,
        entry(AccountData {
            nonce: parent.nonce,
            balance: parent.balance,
            code_hash: parent.bytecode_hash,
        }),
    );
    let (account_policy, storage_policy) = policies();
    let cache = NetworkStateCache::restore(
        accounts,
        HashMap::new(),
        HashMap::new(),
        ANCHOR_BLOCK,
        account_policy,
        storage_policy,
    );
    let anchor = anchor_for(&cache);

    let mut continuous_values = clone_cache(&cache);
    let mut continuous_trie = PartialTrieNodeCache::new();
    assert_eq!(
        try_compute_trustless_state_root(
            proof.clone(),
            &mut continuous_trie,
            &BundleState::default(),
        )
        .unwrap(),
        state_root
    );
    continuous_trie.retain_from_value_cache(&continuous_values);

    let pkg = CacheSnapshotPackage::from_cache(&cache, anchor, &proof);
    let (account_policy, storage_policy) = policies();
    let mut bootstrapped =
        verify_and_restore(pkg, &anchor, state_root, account_policy, storage_policy).unwrap();

    let mut bundle = BundleState::default();
    bundle.state.insert(
        address,
        BundleAccount {
            info: Some(account_info(updated)),
            original_info: Some(account_info(parent)),
            storage: Default::default(),
            status: AccountStatus::Changed,
        },
    );
    let mut accessed = BlockAccessedState::default();
    accessed.accounts.insert(
        address,
        AccountData {
            nonce: updated.nonce,
            balance: updated.balance,
            code_hash: updated.bytecode_hash,
        },
    );

    continuous_values.on_block_executed(ANCHOR_BLOCK + 1, &accessed);
    bootstrapped.value_cache.on_block_executed(ANCHOR_BLOCK + 1, &accessed);
    let continuous_root =
        try_compute_trustless_state_root(MultiProof::default(), &mut continuous_trie, &bundle)
            .unwrap();
    let bootstrapped_root = try_compute_trustless_state_root(
        MultiProof::default(),
        &mut bootstrapped.trie_cache,
        &bundle,
    )
    .unwrap();
    continuous_trie.retain_from_value_cache(&continuous_values);
    bootstrapped.trie_cache.retain_from_value_cache(&bootstrapped.value_cache);

    let (_, expected_root) = state_proof(address, updated, &[], &[]);
    assert_eq!(continuous_root, expected_root);
    assert_eq!(bootstrapped_root, expected_root);
    assert_eq!(bootstrapped.value_cache.cache_root(), continuous_values.cache_root());
    assert_eq!(bootstrapped.trie_cache.cache_root(), continuous_trie.cache_root());
    bootstrapped.trie_cache.validate_against_value_cache(&bootstrapped.value_cache).unwrap();
}
