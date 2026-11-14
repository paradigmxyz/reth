//! Translation of authoritative Reth execution state into PBT snapshots.

#![cfg(feature = "execution")]

use alloy_primitives::{keccak256, Address, B256, U256};
use reth_trie_common::bundle_state::{AccountStatus, BundleAccount, BundleState, StorageSlot};
use reth_trie_pbt::{
    execution::{apply_bundle, ExecutionError},
    AccountData, Blake3Hasher, StateError, StateTree,
};

#[cfg(feature = "parallel")]
use reth_trie_pbt::execution::apply_bundle_parallel;

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
