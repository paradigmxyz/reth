//! Tests for the Commonware LtHash state root helpers.

#![cfg(feature = "lattice-state-root")]

use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_trie::lattice::{LatticeHashState, LatticeRoot, LATTICE_HASH_STATE_BYTES};

#[test]
fn lattice_hash_state_roundtrip() {
    let mut root = LatticeRoot::default();
    root.add_slot(B256::with_last_byte(1), B256::with_last_byte(2), U256::from(3));

    let state = root.state();
    let restored = LatticeRoot::from_state(&state).unwrap();

    assert_eq!(state.as_slice().len(), LATTICE_HASH_STATE_BYTES);
    assert_eq!(root.root(), restored.root());
    assert!(LatticeHashState::from_slice(state.as_slice()).is_ok());
}

#[test]
fn lattice_storage_add_subtract_inverse() {
    let hashed_address = B256::with_last_byte(9);
    let slot = B256::with_last_byte(1);
    let value = U256::from(2);

    let mut root = LatticeRoot::default();
    let empty = root.root();

    root.add_slot(hashed_address, slot, value);
    assert_ne!(root.root(), empty);

    root.subtract_slot(hashed_address, slot, value);
    assert_eq!(root.root(), empty);
}

#[test]
fn lattice_account_add_subtract_inverse() {
    let account =
        Account { nonce: 1, balance: U256::from(2), bytecode_hash: Some(B256::with_last_byte(3)) };
    let hashed_address = B256::with_last_byte(4);

    let mut root = LatticeRoot::default();
    let empty = root.root();

    root.add_account(hashed_address, account);
    assert_ne!(root.root(), empty);

    root.subtract_account(hashed_address, account);
    assert_eq!(root.root(), empty);
}

#[test]
fn lattice_flat_root_is_order_independent() {
    let hashed_address = B256::with_last_byte(9);
    let slot_a = B256::with_last_byte(1);
    let slot_b = B256::with_last_byte(2);

    let mut first = LatticeRoot::default();
    first.add_slot(hashed_address, slot_a, U256::from(1));
    first.add_slot(hashed_address, slot_b, U256::from(2));

    let mut second = LatticeRoot::default();
    second.add_slot(hashed_address, slot_b, U256::from(2));
    second.add_slot(hashed_address, slot_a, U256::from(1));

    assert_eq!(first.root(), second.root());
}

#[test]
fn lattice_flat_root_includes_address_in_storage_element() {
    let hashed_address = B256::with_last_byte(1);
    let other_address = B256::with_last_byte(2);
    let slot = B256::with_last_byte(3);

    let mut first = LatticeRoot::default();
    first.add_slot(hashed_address, slot, U256::from(1));

    let mut second = LatticeRoot::default();
    second.add_slot(other_address, slot, U256::from(1));

    assert_ne!(first.root(), second.root());
}
