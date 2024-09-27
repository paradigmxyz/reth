use alloy_primitives::{B256, U256};
use proptest::prelude::*;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRW},
    tables,
    transaction::DbTxMut,
};
use reth_primitives::StorageEntry;
use reth_provider::{test_utils::create_test_provider_factory, StorageTrieWriter};
use reth_trie::{
    prefix_set::TriePrefixSetsMut, test_utils::storage_root_prehashed, witness::TrieWitness,
    HashedPostState, HashedStorage, StorageRoot, EMPTY_ROOT_HASH,
};
use reth_trie_db::{DatabaseStorageRoot, DatabaseTrieWitness};
use std::collections::{BTreeMap, HashMap};

// #[test]
// fn f() {
//     let hashed_address = B256::random();
//     let factory = create_test_provider_factory();
//     let provider = factory.provider_rw().unwrap();

//     let (_, _, mut storage_trie_nodes) =
//         StorageRoot::from_tx_hashed(provider.tx_ref(),
// hashed_address).root_with_updates().unwrap();

//     provider.write_storage_trie_updates(storage_tries).unwrap();
//     TriePrefixSetsMut { storage_prefix_sets: HashMap::from([(hashed_address,)]) }
// }

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128, ..ProptestConfig::default()
    })]

    // Test that the witness can always be computed.
    #[test]
    fn fuzz_in_execution_witness_storage(init_storage: BTreeMap<B256, U256>, storage_updates: [(bool, BTreeMap<B256, U256>); 10]) {
        let hashed_address = B256::random();
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        // Insert account
        provider.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap().upsert(hashed_address, Default::default()).unwrap();

        // Insert init state into database
        let mut hashed_storage_cursor = provider.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();
        for (hashed_slot, value) in init_storage.clone() {
            hashed_storage_cursor
                .upsert(hashed_address, StorageEntry { key: hashed_slot, value })
                .unwrap();
        }

        // Compute initial storage root and updates
        let (mut previous_storage_root, _, trie_updates) =
            StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address).root_with_updates().unwrap();

        provider.write_storage_trie_updates(&HashMap::from([(hashed_address, trie_updates)])).unwrap();

        let mut storage = init_storage;
        for (is_deleted, mut storage_update) in storage_updates {
            let hashed_storage = HashedStorage::from_iter(is_deleted, storage_update.clone());

            let prefix_set = hashed_storage.construct_prefix_set();
            let witness = TrieWitness::from_tx(provider.tx_ref())
                .with_prefix_sets_mut(TriePrefixSetsMut {
                    storage_prefix_sets: HashMap::from([(hashed_address, prefix_set.clone())]),
                    ..Default::default()
                })
                .compute(HashedPostState {
                    accounts: HashMap::from([(hashed_address, Some(Default::default()))]),
                    storages: HashMap::from([(hashed_address, hashed_storage)]),
                })
                .unwrap();
            assert!(!witness.is_empty());
            if previous_storage_root != EMPTY_ROOT_HASH {
                assert!(witness.contains_key(&previous_storage_root));
            }

            // Insert state updates into database
            if is_deleted && hashed_storage_cursor.seek_exact(hashed_address).unwrap().is_some() {
                hashed_storage_cursor.delete_current_duplicates().unwrap();
            }
            for (hashed_slot, value) in storage_update.clone() {
                hashed_storage_cursor
                    .upsert(hashed_address, StorageEntry { key: hashed_slot, value })
                    .unwrap();
            }

            // Compute root with in-memory trie nodes overlay
            let (storage_root, _, trie_updates) =
                StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address)
                    .with_prefix_set(prefix_set.freeze())
                    .root_with_updates()
                    .unwrap();
            provider.write_storage_trie_updates(&HashMap::from([(hashed_address, trie_updates)])).unwrap();

            // Verify the result
            if is_deleted {
                storage.clear();
            }
            storage.append(&mut storage_update);
            let expected_root = storage_root_prehashed(storage.clone());
            assert_eq!(expected_root, storage_root);

            previous_storage_root = storage_root;
        }
    }
}

#[test]
fn execution_witness_storage() {
    let init_storage = BTreeMap::default();
    let storage_updates = [
        (false, BTreeMap::default()),
        // (false, BTreeMap::from([(B256::with_last_byte(69), U256::ZERO)])),
        // (false, BTreeMap::from([(B256::with_last_byte(42), U256::ZERO)])),
        (false, BTreeMap::from([((B256::with_last_byte(69), U256::from(1)))])),
        (false, BTreeMap::from([(B256::with_last_byte(42), U256::from(2))])),
    ];

    let hashed_address = B256::random();
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();

    // Insert account
    provider
        .tx_ref()
        .cursor_write::<tables::HashedAccounts>()
        .unwrap()
        .upsert(hashed_address, Default::default())
        .unwrap();

    // Insert init state into database
    let mut hashed_storage_cursor =
        provider.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();
    for (hashed_slot, value) in init_storage.clone() {
        hashed_storage_cursor
            .upsert(hashed_address, StorageEntry { key: hashed_slot, value })
            .unwrap();
    }

    // Compute initial storage root and updates
    let (mut previous_storage_root, _, trie_updates) =
        StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address).root_with_updates().unwrap();

    provider.write_storage_trie_updates(&HashMap::from([(hashed_address, trie_updates)])).unwrap();

    let mut storage = init_storage;
    for (is_deleted, mut storage_update) in storage_updates {
        // Insert state updates into database
        let hashed_storage = HashedStorage::from_iter(is_deleted, storage_update.clone());

        let prefix_set = hashed_storage.construct_prefix_set();
        let witness = TrieWitness::from_tx(provider.tx_ref())
            .with_prefix_sets_mut(TriePrefixSetsMut {
                storage_prefix_sets: HashMap::from([(hashed_address, prefix_set.clone())]),
                ..Default::default()
            })
            .compute(HashedPostState {
                accounts: HashMap::from([(hashed_address, Some(Default::default()))]),
                storages: HashMap::from([(hashed_address, hashed_storage)]),
            })
            .unwrap();
        println!("prev storage root {previous_storage_root} witness {:?}", witness);
        assert!(!witness.is_empty());
        if previous_storage_root != EMPTY_ROOT_HASH {
            assert!(witness.contains_key(&previous_storage_root));
        }

        if is_deleted && hashed_storage_cursor.seek_exact(hashed_address).unwrap().is_some() {
            hashed_storage_cursor.delete_current_duplicates().unwrap();
        }
        for (hashed_slot, value) in storage_update.clone() {
            hashed_storage_cursor
                .upsert(hashed_address, StorageEntry { key: hashed_slot, value })
                .unwrap();
        }

        // Compute root with in-memory trie nodes overlay
        let (storage_root, _, trie_updates) =
            StorageRoot::from_tx_hashed(provider.tx_ref(), hashed_address)
                .with_prefix_set(prefix_set.freeze())
                .root_with_updates()
                .unwrap();
        provider
            .write_storage_trie_updates(&HashMap::from([(hashed_address, trie_updates)]))
            .unwrap();

        // Verify the result
        if is_deleted {
            storage.clear();
        }
        storage.append(&mut storage_update);
        let expected_root = storage_root_prehashed(storage.clone());
        assert_eq!(expected_root, storage_root);

        previous_storage_root = storage_root;
    }
}
