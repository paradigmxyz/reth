//! Common functionality using in Trie DB tests.

use alloy_primitives::{keccak256, Address, B256, U256};
use reth_db::tables;
use reth_db_api::transaction::DbTxMut;
use reth_primitives::{Account, StorageEntry};
use std::collections::BTreeMap;

/// State type alias representing the full state tree mapping.
#[allow(dead_code)]
pub(crate) type State = BTreeMap<Address, (Account, BTreeMap<B256, U256>)>;

/// Insert an account and its storage into the database.
#[allow(dead_code)]
pub(crate) fn insert_account(
    tx: &impl DbTxMut,
    address: Address,
    account: Account,
    storage: &BTreeMap<B256, U256>,
) {
    let hashed_address = keccak256(address);
    tx.put::<tables::HashedAccounts>(hashed_address, account).unwrap();
    insert_storage(tx, hashed_address, storage);
}

/// Insert storage entries for a given hashed address into the database.
#[allow(dead_code)]
fn insert_storage(tx: &impl DbTxMut, hashed_address: B256, storage: &BTreeMap<B256, U256>) {
    for (k, v) in storage {
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: keccak256(k), value: *v },
        )
        .unwrap();
    }
}
