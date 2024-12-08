#![allow(missing_docs)]

use alloy_primitives::{Address, Uint, B256, U256};
use proptest::{prelude::ProptestConfig, proptest};
use proptest_arbitrary_interop::arb;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRW},
    tables,
    transaction::DbTxMut,
};
use reth_primitives::{Account, StorageEntry};
use reth_provider::test_utils::create_test_provider_factory;
use reth_scroll_state_commitment::{
    test_utils::{b256_clear_last_byte, b256_reverse_bits, u256_clear_msb},
    PoseidonKeyHasher, StateRoot, StorageRoot,
};

use reth_scroll_state_commitment::test_utils::b256_clear_first_byte;
use reth_trie::{
    trie_cursor::InMemoryTrieCursorFactory, updates::TrieUpdates, HashedPostState, HashedStorage,
    KeyHasher,
};
use reth_trie_db::{DatabaseStateRoot, DatabaseStorageRoot, DatabaseTrieCursorFactory};
use std::collections::BTreeMap;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 6, ..ProptestConfig::default()
    })]

    #[test]
    fn fuzz_in_memory_account_nodes(mut init_state: BTreeMap<B256, (u32, U256, Option<(B256, B256, u64)>)>, state_updates: [BTreeMap<B256, Option<U256>>; 10]) {
        let init_state: BTreeMap<B256, Account> = init_state.into_iter().map(|(hashed_address, (nonce, balance, code))| {
            let hashed_address = b256_clear_last_byte(hashed_address);
            let balance = u256_clear_msb(balance);
            #[cfg(feature = "scroll")]
            let account_extension = code.map( |(_, code_hash, code_size)| (code_size, b256_clear_first_byte(code_hash)).into()).or_else(|| Some(Default::default()));
            let account = Account { balance, nonce: nonce.into(), bytecode_hash: code.as_ref().map(|(code_hash, _, _)| *code_hash),
                #[cfg(feature = "scroll")]
                account_extension
            };
            (hashed_address, account)
        }).collect();
        let state_updates: Vec<BTreeMap<_, _>> = state_updates.into_iter().map(|update| {
            update.into_iter().map(|(hashed_address, update)| {
                let hashed_address = b256_clear_last_byte(hashed_address);
                let account = update.map(|balance| Account {
                    balance: u256_clear_msb(balance),
                    #[cfg(feature = "scroll")]
                    account_extension: Some(Default::default()),
                    ..Default::default() });
                (hashed_address, account)
            }).collect::<BTreeMap<_, _>>()
        }).collect();


        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();
        let mut hashed_account_cursor = provider.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        // Insert init state into database
        for (hashed_address, account) in init_state.clone() {
            hashed_account_cursor.upsert(b256_reverse_bits(hashed_address), account).unwrap();
        }

        // Compute initial root and updates
        let (_, mut trie_nodes) = StateRoot::from_tx(provider.tx_ref())
            .root_with_updates()
            .unwrap();

        let mut state = init_state;
        for state_update in state_updates {
            // Insert state updates into database
            let mut hashed_state = HashedPostState::default();
            for (hashed_address, account) in state_update {
                if let Some(account) = account {
                    hashed_account_cursor.upsert(b256_reverse_bits(hashed_address), account).unwrap();
                    hashed_state.accounts.insert(b256_reverse_bits(hashed_address), Some(account));
                    state.insert(hashed_address, account);
                } else {
                    hashed_state.accounts.insert(b256_reverse_bits(hashed_address), None);
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
            let expected_root = reth_scroll_state_commitment::test_utils::state_root(
                state.iter().map(|(key, account)| (*key, (*account, std::iter::empty())))
            );
            assert_eq!(expected_root.0, state_root.0);

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
        let init_storage: BTreeMap<B256, U256> = init_storage.into_iter().map(|(slot, value)| {
            let hashed_slot = b256_clear_last_byte(slot);
            hashed_storage_cursor
                .upsert(hashed_address, StorageEntry { key: b256_reverse_bits(hashed_slot), value })
                .unwrap();
            (hashed_slot, value)
        }).collect();
        let storage_updates: Vec<(bool, BTreeMap<B256, U256>)> = storage_updates.into_iter().map(|(is_deleted, updates)| {
            let updates = updates.into_iter().map(|(slot, value)| {
                let slot = b256_clear_last_byte(slot);
                (slot, value)
            }).collect();
            (is_deleted, updates)
        }).collect();

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
                    .upsert(hashed_address, StorageEntry { key: b256_reverse_bits(hashed_slot), value })
                    .unwrap();
                hashed_storage.storage.insert(b256_reverse_bits(hashed_slot), value);
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
            let expected_root = reth_scroll_state_commitment::test_utils::storage_root(storage.clone());
            assert_eq!(expected_root, storage_root);
        }
    }
}

#[test]
fn test_basic_state_root_with_updates_succeeds() {
    let address_1 = Address::with_last_byte(0);
    let address_2 = Address::with_last_byte(3);
    let address_3 = Address::with_last_byte(7);
    let account_1 = Account {
        balance: Uint::from(1),
        #[cfg(feature = "scroll")]
        account_extension: Some(Default::default()),
        ..Default::default()
    };
    let account_2 = Account {
        balance: Uint::from(2),
        #[cfg(feature = "scroll")]
        account_extension: Some(Default::default()),
        ..Default::default()
    };
    let account_3 = Account {
        balance: Uint::from(3),
        #[cfg(feature = "scroll")]
        account_extension: Some(Default::default()),
        ..Default::default()
    };

    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    insert_account(tx.tx_ref(), address_1, account_1, &Default::default());
    insert_account(tx.tx_ref(), address_2, account_2, &Default::default());
    insert_account(tx.tx_ref(), address_3, account_3, &Default::default());

    tx.commit().unwrap();

    let tx = factory.provider_rw().unwrap();
    let (_root, _updates) = StateRoot::from_tx(tx.tx_ref()).root_with_updates().unwrap();
}

fn insert_account(
    tx: &impl DbTxMut,
    address: Address,
    account: Account,
    storage: &BTreeMap<B256, U256>,
) {
    let hashed_address = PoseidonKeyHasher::hash_key(address);
    tx.put::<tables::HashedAccounts>(hashed_address, account).unwrap();
    insert_storage(tx, hashed_address, storage);
}

fn insert_storage(tx: &impl DbTxMut, hashed_address: B256, storage: &BTreeMap<B256, U256>) {
    for (k, v) in storage {
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: PoseidonKeyHasher::hash_key(k), value: *v },
        )
        .unwrap();
    }
}
#[test]
fn arbitrary_storage_root() {
    proptest!(ProptestConfig::with_cases(10), |(item in arb::<(Address, std::collections::BTreeMap<B256, U256>)>())| {
        let (address, storage) = item;

        let hashed_address = PoseidonKeyHasher::hash_key(address);
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap();
        let storage: BTreeMap<B256, U256> = storage.into_iter().map(|(key, value)|  {
            let key = b256_clear_last_byte(key);
            tx.tx_ref().put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key, value },
            )
            .unwrap();
            (b256_reverse_bits(key), value)
        }).collect();
        tx.commit().unwrap();

        let tx =  factory.provider_rw().unwrap();
        let got = StorageRoot::from_tx(tx.tx_ref(), address).root().unwrap();
        let expected = reth_scroll_state_commitment::test_utils::storage_root(storage.into_iter());
        assert_eq!(expected, got);
    });
}
