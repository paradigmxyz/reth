//! Tests for the Commonware LtHash state root helpers.

#![cfg(feature = "lattice-state-root")]

use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_trie::lattice::{
    LatticeHashState, LatticeStateRoot, LatticeStorageRoot, LATTICE_HASH_STATE_BYTES,
};

#[test]
fn lattice_hash_state_roundtrip() {
    let mut root = LatticeStorageRoot::default();
    root.add_slot(B256::with_last_byte(1), U256::from(2));

    let state = root.state();
    let restored = LatticeStorageRoot::from_state(&state).unwrap();

    assert_eq!(state.as_slice().len(), LATTICE_HASH_STATE_BYTES);
    assert_eq!(root.root(), restored.root());
    assert!(LatticeHashState::from_slice(state.as_slice()).is_ok());
}

#[test]
fn lattice_storage_add_subtract_inverse() {
    let slot = B256::with_last_byte(1);
    let value = U256::from(2);

    let mut root = LatticeStorageRoot::default();
    let empty = root.root();

    root.add_slot(slot, value);
    assert_ne!(root.root(), empty);

    root.subtract_slot(slot, value);
    assert_eq!(root.root(), empty);
    assert!(root.is_zero());
}

#[test]
fn lattice_account_add_subtract_inverse() {
    let account =
        Account { nonce: 1, balance: U256::from(2), bytecode_hash: Some(B256::with_last_byte(3)) };
    let hashed_address = B256::with_last_byte(4);
    let storage_root = B256::with_last_byte(5);

    let mut root = LatticeStateRoot::default();
    let empty = root.root();

    root.add_account(hashed_address, account, storage_root);
    assert_ne!(root.root(), empty);

    root.subtract_account(hashed_address, account, storage_root);
    assert_eq!(root.root(), empty);
}

#[test]
fn lattice_storage_root_is_order_independent() {
    let slot_a = B256::with_last_byte(1);
    let slot_b = B256::with_last_byte(2);

    let mut first = LatticeStorageRoot::default();
    first.add_slot(slot_a, U256::from(1));
    first.add_slot(slot_b, U256::from(2));

    let mut second = LatticeStorageRoot::default();
    second.add_slot(slot_b, U256::from(2));
    second.add_slot(slot_a, U256::from(1));

    assert_eq!(first.root(), second.root());
}

#[test]
fn lattice_state_root_includes_nested_storage_root() {
    let hashed_address = B256::with_last_byte(1);
    let account = Account { nonce: 1, balance: U256::from(2), bytecode_hash: None };

    let mut storage_a = LatticeStorageRoot::default();
    storage_a.add_slot(B256::with_last_byte(1), U256::from(1));

    let mut storage_b = LatticeStorageRoot::default();
    storage_b.add_slot(B256::with_last_byte(1), U256::from(2));

    let mut first = LatticeStateRoot::default();
    first.add_account(hashed_address, account, storage_a.root());

    let mut second = LatticeStateRoot::default();
    second.add_account(hashed_address, account, storage_b.root());

    assert_ne!(first.root(), second.root());
}
