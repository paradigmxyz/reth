//! Translation of authoritative Reth execution state into PBT snapshots.

#![cfg(feature = "execution")]

use alloy_primitives::{keccak256, Address, B256, KECCAK256_EMPTY, U256};
use reth_primitives_traits::Bytecode;
use reth_trie_common::bundle_state::{AccountStatus, BundleAccount, BundleState, StorageSlot};
use reth_trie_pbt::{
    execution::{apply_bundle, ExecutionError},
    AccountData, Blake3Hasher, StateError, StateTree,
};

#[cfg(feature = "parallel")]
use reth_trie_pbt::execution::apply_bundle_parallel;

#[cfg(feature = "persistence")]
use reth_trie_pbt::{
    execution::{ExecutionParent, ExecutionState},
    migration::{mpt_root, DecodeLimits},
    persistence::SnapshotStore,
};

#[test]
fn transfer_and_storage_output_matches_direct_encoding() {
    let a = Address::repeat_byte(1);
    let mut bundle = BundleState::default();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::InMemoryChange,
    );
    account.info.as_mut().unwrap().balance = U256::from(100);
    account.info.as_mut().unwrap().nonce = 1;
    account.storage.insert(U256::from(64), StorageSlot::new_changed(U256::ZERO, U256::from(7)));
    bundle.state.insert(a, account);
    let parent = StateTree::<Blake3Hasher>::new();
    let state = apply_bundle(&parent, &bundle).unwrap();
    let mut direct = parent.clone();
    direct
        .set_account(
            a,
            AccountData { nonce: 1, balance: U256::from(100), code: Default::default() },
        )
        .unwrap();
    direct.set_storage(a, U256::from(64), B256::from(U256::from(7))).unwrap();
    assert_eq!(state.root(), direct.root());
    assert_eq!(parent.root(), B256::ZERO);
}

#[test]
fn destruction_and_recreation_wipe_parent_storage() {
    let a = Address::repeat_byte(1);
    let mut parent = StateTree::<Blake3Hasher>::new();
    parent
        .set_account(a, AccountData { nonce: 1, balance: U256::from(5), code: Default::default() })
        .unwrap();
    parent.set_storage(a, U256::from(63), B256::repeat_byte(1)).unwrap();
    parent.set_storage(a, U256::from(256), B256::repeat_byte(1)).unwrap();
    let mut bundle = BundleState::default();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::DestroyedChanged,
    );
    account.info.as_mut().unwrap().balance = U256::from(6);
    bundle.state.insert(a, account);
    let state = apply_bundle(&parent, &bundle).unwrap();
    assert_eq!(state.storage(a, U256::from(63)), B256::ZERO);
    assert_eq!(state.storage(a, U256::from(256)), B256::ZERO);
    bundle.state.get_mut(&a).unwrap().info = None;
    assert_eq!(apply_bundle(&state, &bundle).unwrap().root(), B256::ZERO);
}

#[test]
fn missing_new_bytecode_fails_without_modifying_parent() {
    let a = Address::repeat_byte(1);
    let mut bundle = BundleState::default();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::InMemoryChange,
    );
    account.info.as_mut().unwrap().code = None;
    account.info.as_mut().unwrap().code_hash = B256::repeat_byte(1);
    bundle.state.insert(a, account);
    let parent = StateTree::<Blake3Hasher>::new();
    assert!(matches!(
        apply_bundle(&parent, &bundle),
        Err(ExecutionError::MissingCode(hash)) if hash == B256::repeat_byte(1)
    ));
    assert_eq!(parent.root(), B256::ZERO);
}

#[test]
fn unchanged_code_is_recovered_from_parent_and_storage_zero_is_deleted() {
    let address = Address::repeat_byte(1);
    let code = vec![0x60, 0x01, 0x00];
    let code_hash = keccak256(&code);
    let mut parent = StateTree::<Blake3Hasher>::new();
    parent
        .set_account(address, AccountData { nonce: 1, balance: U256::from(5), code: code.into() })
        .unwrap();
    parent.set_storage(address, U256::from(63), B256::repeat_byte(1)).unwrap();
    parent.set_storage(address, U256::from(256), B256::repeat_byte(2)).unwrap();
    let parent_root = parent.root();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::Changed,
    );
    let info = account.info.as_mut().unwrap();
    info.nonce = 2;
    info.balance = U256::from(6);
    info.code_hash = code_hash;
    info.code = None;
    account
        .storage
        .insert(U256::from(63), StorageSlot::new_changed(B256::repeat_byte(1).into(), U256::ZERO));
    let mut bundle = BundleState::default();
    bundle.state.insert(address, account);
    let state = apply_bundle(&parent, &bundle).unwrap();
    let mut expected = parent.clone();
    let mut updated = parent.account(address).unwrap().clone();
    updated.nonce = 2;
    updated.balance = U256::from(6);
    expected.set_account(address, updated).unwrap();
    expected.set_storage(address, U256::from(63), B256::ZERO).unwrap();
    assert_eq!(state.root(), expected.root());
    assert_eq!(parent.root(), parent_root);
    assert_eq!(state.storage(address, U256::from(256)), B256::repeat_byte(2));
}

#[test]
fn inconsistent_embedded_code_is_rejected_instead_of_using_parent_code() {
    let address = Address::repeat_byte(1);
    let code = vec![0x60, 0x01, 0x00];
    let code_hash = keccak256(&code);
    let mut parent = StateTree::<Blake3Hasher>::new();
    parent
        .set_account(address, AccountData { nonce: 1, balance: U256::from(5), code: code.into() })
        .unwrap();
    let root = parent.root();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::Changed,
    );
    let info = account.info.as_mut().unwrap();
    info.code_hash = code_hash;
    info.code = Some(Default::default());
    let mut bundle = BundleState::default();
    bundle.state.insert(address, account);
    assert!(matches!(
        apply_bundle(&parent, &bundle),
        Err(ExecutionError::MissingCode(hash)) if hash == code_hash
    ));
    assert_eq!(parent.root(), root);
}

#[test]
fn nonempty_code_with_an_empty_hash_is_rejected() {
    for hash in [KECCAK256_EMPTY, B256::ZERO] {
        for embedded in [false, true] {
            let address = Address::repeat_byte(1);
            let mut account = BundleAccount::new(
                None,
                Some(Default::default()),
                Default::default(),
                AccountStatus::InMemoryChange,
            );
            let info = account.info.as_mut().unwrap();
            info.nonce = 1;
            info.code_hash = hash;
            let code = Bytecode::new_raw(vec![0x60, 0x01].into()).0;
            info.code = embedded.then(|| code.clone());
            let mut bundle = BundleState::default();
            bundle.state.insert(address, account);
            if !embedded {
                bundle.contracts.insert(hash, code);
            }
            let parent = StateTree::<Blake3Hasher>::new();
            assert!(matches!(
                apply_bundle(&parent, &bundle),
                Err(ExecutionError::MissingCode(actual)) if actual == hash
            ));
            assert_eq!(parent.root(), B256::ZERO);
            #[cfg(feature = "persistence")]
            {
                let directory = tempfile::tempdir().unwrap();
                let mut store = SnapshotStore::open_with_limits(
                    directory.path(),
                    DecodeLimits::default(),
                    DecodeLimits::native(),
                )
                .unwrap();
                store.set_cache_budget(0).unwrap();
                let anchor = B256::repeat_byte(1);
                store.save_block_with_number(anchor, 0, &parent).unwrap();
                let cold = store.execution_parent(anchor, Some(parent.root())).unwrap();
                assert!(matches!(&cold, ExecutionParent::Disk(_)));
                assert!(matches!(
                    cold.apply_bundle(&bundle, false),
                    Err(ExecutionError::MissingCode(actual)) if actual == hash
                ));
                assert_eq!(store.verify_block(anchor).unwrap(), parent.root());
            }
            bundle.state.get_mut(&address).unwrap().info.as_mut().unwrap().code =
                embedded.then(Default::default);
            if !embedded {
                bundle.contracts.insert(hash, Default::default());
            }
            assert!(apply_bundle(&parent, &bundle)
                .unwrap()
                .account(address)
                .unwrap()
                .code
                .is_empty());
        }
    }
}

#[test]
fn account_encoding_failure_preserves_parent_storage_and_code() {
    let address = Address::repeat_byte(1);
    let mut parent = StateTree::<Blake3Hasher>::new();
    parent
        .set_account(
            address,
            AccountData { nonce: 1, balance: U256::from(5), code: vec![1].into() },
        )
        .unwrap();
    parent.set_storage(address, U256::MAX, B256::repeat_byte(1)).unwrap();
    let root = parent.root();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::DestroyedChanged,
    );
    account.info.as_mut().unwrap().balance = U256::MAX;
    let mut bundle = BundleState::default();
    bundle.state.insert(address, account);
    assert!(matches!(
        apply_bundle(&parent, &bundle),
        Err(ExecutionError::State(StateError::BalanceOverflow))
    ));
    assert_eq!(parent.root(), root);
    assert_eq!(parent.storage(address, U256::MAX), B256::repeat_byte(1));
    assert_eq!(parent.account(address).unwrap().code.as_ref(), &[1]);
}

#[cfg(feature = "parallel")]
#[test]
fn dense_parallel_execution_matches_serial_and_preserves_failed_parent() {
    let parent = StateTree::<Blake3Hasher>::new();
    let mut bundle = BundleState::default();
    for index in 0u64..4096 {
        let address = Address::from_word(B256::from(U256::from(index)));
        let mut account = BundleAccount::new(
            None,
            Some(Default::default()),
            Default::default(),
            AccountStatus::InMemoryChange,
        );
        let info = account.info.as_mut().unwrap();
        info.nonce = 1;
        info.balance = U256::from(index + 1);
        account.storage.insert(U256::from(64), StorageSlot::new_changed(U256::ZERO, U256::from(1)));
        bundle.state.insert(address, account);
    }
    let serial = apply_bundle(&parent, &bundle).unwrap();
    let parallel = apply_bundle_parallel(&parent, &bundle).unwrap();
    assert_eq!(parallel.root(), serial.root());
    assert_eq!(
        parallel.tree().entries().collect::<Vec<_>>(),
        serial.tree().entries().collect::<Vec<_>>()
    );
    assert_eq!(parent.root(), B256::ZERO);
    let before = parallel.root();
    bundle.state.get_mut(&Address::ZERO).unwrap().info.as_mut().unwrap().balance = U256::MAX;
    assert!(matches!(
        apply_bundle_parallel(&parallel, &bundle),
        Err(ExecutionError::State(StateError::BalanceOverflow))
    ));
    assert_eq!(parallel.root(), before);
}

#[cfg(all(feature = "parallel", feature = "persistence"))]
#[test]
fn cold_dense_execution_uses_serial_overlay_and_matches_both_eager_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut parent = StateTree::<Blake3Hasher>::new();
    let mut bundle = BundleState::default();
    for index in 0u64..4096 {
        let address = Address::from_word(B256::from(U256::from(index)));
        parent.set_account(address, AccountData { nonce: 1, ..Default::default() }).unwrap();
        parent.set_storage(address, U256::from(64), B256::repeat_byte(1)).unwrap();
        let mut account = BundleAccount::new(
            None,
            Some(Default::default()),
            Default::default(),
            AccountStatus::Changed,
        );
        account.info.as_mut().unwrap().nonce = 2;
        account.storage.insert(
            U256::from(64),
            StorageSlot::new_changed(U256::from_be_bytes([1; 32]), U256::from(index + 1)),
        );
        bundle.state.insert(address, account);
    }
    let anchor = B256::repeat_byte(1);
    let child = B256::repeat_byte(2);
    let limits = DecodeLimits::default();
    let native = DecodeLimits::native();
    let mut store = SnapshotStore::open_with_limits(dir.path(), limits, native).unwrap();
    store.set_cache_budget(0).unwrap();
    store.save_block_with_number(anchor, 0, &parent).unwrap();
    let disk = store.execution_parent(anchor, Some(parent.root())).unwrap();
    assert!(matches!(&disk, ExecutionParent::Disk(_)));

    let eager = ExecutionParent::Eager(parent);
    let serial = eager.apply_bundle(&bundle, false).unwrap();
    let parallel = eager.apply_bundle(&bundle, true).unwrap();
    let overlay = disk.apply_bundle(&bundle, true).unwrap();
    assert!(matches!(&overlay, ExecutionState::Disk(_)));
    assert!(!overlay.uses_parallel_batch(true));
    assert!(!serial.uses_parallel_batch(false));
    assert!(parallel.uses_parallel_batch(true));
    assert_eq!(overlay.root(), serial.root());
    assert_eq!(overlay.root(), parallel.root());
    assert_eq!(overlay.leaf_count(), serial.leaf_count());
    assert_eq!(overlay.leaf_count(), parallel.leaf_count());

    store.save_execution_block(child, 1, &overlay).unwrap();
    drop((disk, overlay, store));
    let reopened = SnapshotStore::<Blake3Hasher>::open_with_limits(dir.path(), limits, native)
        .unwrap()
        .load_block(child)
        .unwrap();
    assert_eq!(reopened.root(), serial.root());
    let ExecutionState::Eager(serial) = serial else { panic!("expected eager execution") };
    assert!(reopened.tree().entries().eq(serial.tree().entries()));
}

#[cfg(feature = "persistence")]
#[test]
fn cold_execution_publishes_only_accepted_overlays_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let a = Address::repeat_byte(1);
    let b = Address::repeat_byte(2);
    let code = alloy_primitives::Bytes::from_static(&[0x60, 0x01, 0x00]);
    let mut parent = StateTree::<Blake3Hasher>::new();
    for address in [a, b] {
        parent
            .set_account(
                address,
                AccountData { code: code.clone(), nonce: 1, ..Default::default() },
            )
            .unwrap();
        parent.set_storage(address, U256::from(256), B256::repeat_byte(1)).unwrap();
    }
    let anchor = B256::repeat_byte(1);
    let child = B256::repeat_byte(2);
    let discarded = B256::repeat_byte(3);
    let limits = DecodeLimits::default();
    let native = DecodeLimits::native();
    let store = SnapshotStore::open_with_limits(dir.path(), limits, native).unwrap();
    store.save_block_with_number(anchor, 0, &parent).unwrap();
    drop(store);

    let mut store =
        SnapshotStore::<Blake3Hasher>::open_with_limits(dir.path(), limits, native).unwrap();
    store.set_cache_budget(0).unwrap();
    let (restored, shadow) = store.execution_parent_with_shadow(anchor, parent.root()).unwrap();
    assert!(matches!(restored, ExecutionParent::Eager(_)));
    assert_eq!(shadow, mpt_root(&parent));
    drop(restored);
    let (lazy, cached_shadow) = store.execution_parent_with_shadow(anchor, parent.root()).unwrap();
    assert!(matches!(lazy, ExecutionParent::Disk(_)));
    assert_eq!(cached_shadow, shadow);
    let mut bundle = BundleState::default();
    let mut account = BundleAccount::new(
        None,
        Some(Default::default()),
        Default::default(),
        AccountStatus::Changed,
    );
    let info = account.info.as_mut().unwrap();
    info.nonce = 2;
    info.balance = U256::from(5);
    info.code_hash = keccak256(&code);
    info.code = None;
    account.storage.insert(U256::from(256), StorageSlot::new_changed(U256::from(1), U256::ZERO));
    bundle.state.insert(a, account);
    let expected = apply_bundle(&parent, &bundle).unwrap();
    let state = lazy.apply_bundle(&bundle, true).unwrap();
    assert!(matches!(state, ExecutionState::Disk(_)));
    assert_eq!(state.root(), expected.root());
    assert_eq!(state.leaf_count(), expected.tree().len());
    store.cache_execution_candidate(&state).unwrap();
    assert!(!store.contains_block(child).unwrap());
    assert!(!store.contains_block(discarded).unwrap());
    store.save_execution_block(child, 1, &state).unwrap();
    assert_eq!(store.block_number(child).unwrap(), Some(1));

    bundle.state.get_mut(&a).unwrap().info = None;
    let removal = lazy.apply_bundle(&bundle, false).unwrap();
    assert!(matches!(removal, ExecutionState::Eager(_)));
    assert_eq!(removal.root(), apply_bundle(&parent, &bundle).unwrap().root());
    drop((lazy, state, store));
    let reopened =
        SnapshotStore::<Blake3Hasher>::open_with_limits(dir.path(), limits, native).unwrap();
    assert_eq!(reopened.load_block(child).unwrap().root(), expected.root());
    assert_eq!(reopened.load_block(anchor).unwrap().root(), parent.root());
    assert!(!reopened.contains_block(discarded).unwrap());
}
