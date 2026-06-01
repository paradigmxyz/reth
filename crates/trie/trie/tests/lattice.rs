//! Tests for the Commonware LtHash state root helpers.

#![cfg(feature = "lattice-state-root")]

use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_trie::lattice::{LatticeStateRoot, LatticeStorageRoot};

#[test]
fn storage_root_is_order_independent() {
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
fn state_root_includes_nested_storage_root() {
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
