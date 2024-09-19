#![allow(missing_docs)]

use alloy_primitives::{B256, U256};
use proptest::prelude::*;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRW},
    tables,
    transaction::DbTxMut,
};
use reth_primitives::{Account, StorageEntry};
use reth_provider::test_utils::create_test_provider_factory;
use reth_trie::{
    test_utils::{state_root_prehashed, storage_root_prehashed},
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdates,
    HashedPostState, HashedStorage, StateRoot, StorageRoot,
};
use reth_trie_db::{DatabaseStateRoot, DatabaseStorageRoot, DatabaseTrieCursorFactory};
use std::collections::BTreeMap;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128, ..ProptestConfig::default()
    })]

    #[test]
    fn fuzz_in_memory_account_nodes(mut init_state: BTreeMap<B256, U256>, state_updates: [BTreeMap<B256, Option<U256>>; 10]) {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut hashed_account_cursor = provider.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        // Insert init state into database
        for (hashed_address, balance) in init_state.clone() {
            hashed_account_cursor.upsert(hashed_address, Account { balance, ..Default::default() }).unwrap();
        }

        // Compute initial root and updates
        let (_, mut trie_nodes) = StateRoot::from_tx(provider.tx_ref())
            .root_with_updates()
            .unwrap();

        let mut state = init_state;
        for state_update in state_updates {
             // Insert state updates into database
            let mut hashed_state = HashedPostState::default();
            for (hashed_address, balance) in state_update {
                if let Some(balance) = balance {
                    let account = Account { balance, ..Default::default() };
                    hashed_account_cursor.upsert(hashed_address, account).unwrap();
                    hashed_state.accounts.insert(hashed_address, Some(account));
                    state.insert(hashed_address, balance);
                } else {
                    hashed_state.accounts.insert(hashed_address, None);
                    state.remove(&hashed_address);
                }
            }

            // Compute root with in-memory trie nodes overlay
            let (state_root, trie_updates) = StateRoot::from_tx(provider.tx_ref())
                .with_prefix_sets(hashed_state.construct_prefix_sets().freeze())
                .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                    DatabaseTrieCursorFactory::new(provider.tx_ref()), &trie_nodes.clone().into_sorted())
                )
                .root_with_updates()
                .unwrap();

            trie_nodes.extend(trie_updates);

            // Verify the result
            let expected_root = state_root_prehashed(
                state.iter().map(|(&key, &balance)| (key, (Account { balance, ..Default::default() }, std::iter::empty())))
            );
            assert_eq!(expected_root, state_root);
        }
    }

    #[test]
    fn fuzz_in_memory_storage_nodes(mut init_storage: BTreeMap<B256, U256>, storage_updates: [(bool, BTreeMap<B256, U256>); 10]) {
        let hashed_address = B256::random();
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut hashed_storage_cursor =
            provider.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();

        // Insert init state into database
        for (hashed_slot, value) in init_storage.clone() {
            hashed_storage_cursor
                .upsert(hashed_address, StorageEntry { key: hashed_slot, value })
                .unwrap();
        }

        // Compute initial storage root and updates
        let (_, _, mut storage_trie_nodes) =
            StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address).root_with_updates().unwrap();

        let mut storage = init_storage;
        for (is_deleted, mut storage_update) in storage_updates {
            // Insert state updates into database
            if is_deleted && hashed_storage_cursor.seek_exact(hashed_address).unwrap().is_some() {
                hashed_storage_cursor.delete_current_duplicates().unwrap();
            }
            let mut hashed_storage = HashedStorage::new(is_deleted);
            for (hashed_slot, value) in storage_update.clone() {
                hashed_storage_cursor
                    .upsert(hashed_address, StorageEntry { key: hashed_slot, value })
                    .unwrap();
                hashed_storage.storage.insert(hashed_slot, value);
            }

            // Compute root with in-memory trie nodes overlay
            let mut trie_nodes = TrieUpdates::default();
            trie_nodes.insert_storage_updates(hashed_address, storage_trie_nodes.clone());
            let (storage_root, _, trie_updates) =
                StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address)
                    .with_prefix_set(hashed_storage.construct_prefix_set().freeze())
                    .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
                        DatabaseTrieCursorFactory::new(provider.tx_ref()),
                        &trie_nodes.into_sorted(),
                    ))
                    .root_with_updates()
                    .unwrap();

            storage_trie_nodes.extend(trie_updates);

            // Verify the result
            if is_deleted {
                storage.clear();
            }
            storage.append(&mut storage_update);
            let expected_root = storage_root_prehashed(storage.clone());
            assert_eq!(expected_root, storage_root);
        }
    }
}
