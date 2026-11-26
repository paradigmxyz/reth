#![allow(missing_docs, clippy::clone_on_copy)]

use alloy_consensus::EMPTY_ROOT_HASH;
use alloy_primitives::{
    address, b256,
    hex_literal::hex,
    keccak256,
    map::{B256Set, HashMap},
    Address, B256, U256,
};
use alloy_rlp::Encodable;
use proptest::{
    collection, option,
    prelude::{any, ProptestConfig},
    prop_oneof, proptest,
    sample::Index,
    strategy::{Just, Strategy},
};
use proptest_arbitrary_interop::arb;
use reth_db::{tables, test_utils::TempDatabase, DatabaseEnv};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::{
    providers::ProviderNodeTypes,
    test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    DatabaseProviderRW, ProviderFactory, StateWriter, StorageTrieWriter, TrieWriter,
};
use reth_storage_api::StorageSettingsCache;
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    test_utils::{state_root, state_root_prehashed, storage_root, storage_root_prehashed},
    triehash::KeccakHasher,
    updates::{StorageTrieUpdates, TrieUpdatesSorted},
    BranchNodeCompact, HashBuilder, HashedPostState, IntermediateStateRootState, Nibbles,
    StateRoot, StateRootProgress, StorageRoot, TrieMask,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseStorageRoot, DatabaseStorageTrieCursor,
    DatabaseTrieCursorFactory, LegacyKeyAdapter, MemcmpKeyAdapter, PackedKeyAdapter,
    StorageTrieEntryLike, TrieTableAdapter,
};
use std::{cell::Cell, collections::BTreeMap, ops::Mul, str::FromStr, sync::Arc};

type DbStateRoot<'a, TX, A> =
    StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
type DbStorageRoot<'a, TX, A> =
    StorageRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

fn insert_account(
    tx: &impl DbTxMut,
    address: Address,
    account: Account,
    storage: &BTreeMap<B256, U256>,
) {
    let hashed_address = keccak256(address);
    tx.put::<tables::HashedAccounts>(hashed_address, account).unwrap();
    insert_storage(tx, hashed_address, storage);
}

fn insert_storage(tx: &impl DbTxMut, hashed_address: B256, storage: &BTreeMap<B256, U256>) {
    for (k, v) in storage {
        tx.put::<tables::HashedStorages>(
            hashed_address,
            StorageEntry { key: keccak256(k), value: *v },
        )
        .unwrap();
    }
}

fn incremental_vs_full_root(inputs: &[&str], modified: &str) {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    let hashed_address = B256::with_last_byte(1);

    let mut hashed_storage_cursor =
        tx.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
    let data = inputs.iter().map(|x| B256::from_str(x).unwrap());
    let value = U256::ZERO;
    for key in data {
        hashed_storage_cursor.upsert(hashed_address, &StorageEntry { key, value }).unwrap();
    }

    reth_trie_db::with_adapter!(tx, |A| {
        // Generate the intermediate nodes on the receiving end of the channel
        let (_, _, trie_updates) =
            DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address)
                .root_with_updates()
                .unwrap();

        // 1. Some state transition happens, update the hashed storage to the new value
        let modified_key = B256::from_str(modified).unwrap();
        let value = U256::ONE;
        if hashed_storage_cursor.seek_by_key_subkey(hashed_address, modified_key).unwrap().is_some()
        {
            hashed_storage_cursor.delete_current().unwrap();
        }
        hashed_storage_cursor
            .upsert(hashed_address, &StorageEntry { key: modified_key, value })
            .unwrap();

        // 2. Calculate full merkle root
        let loader = DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address);
        let modified_root = loader.root().unwrap();

        // Update the intermediate roots table so that we can run the incremental verification
        tx.write_storage_trie_updates_sorted(core::iter::once((
            &hashed_address,
            &trie_updates.into_sorted(),
        )))
        .unwrap();

        // 3. Calculate the incremental root
        let mut storage_changes = PrefixSetMut::default();
        storage_changes.insert(Nibbles::unpack(modified_key));
        let loader = DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address)
            .with_prefix_set(storage_changes.freeze());
        let incremental_root = loader.root().unwrap();

        assert_eq!(modified_root, incremental_root);
    });
}

#[test]
fn branch_node_child_changes() {
    incremental_vs_full_root(
        &[
            "1000000000000000000000000000000000000000000000000000000000000000",
            "1100000000000000000000000000000000000000000000000000000000000000",
            "1110000000000000000000000000000000000000000000000000000000000000",
            "1200000000000000000000000000000000000000000000000000000000000000",
            "1220000000000000000000000000000000000000000000000000000000000000",
            "1320000000000000000000000000000000000000000000000000000000000000",
        ],
        "1200000000000000000000000000000000000000000000000000000000000000",
    );
}

#[test]
fn arbitrary_storage_root() {
    proptest!(ProptestConfig::with_cases(10), |(item in arb::<(Address, std::collections::BTreeMap<B256, U256>)>())| {
        let (address, storage) = item;

        let hashed_address = keccak256(address);
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap();
        for (key, value) in &storage {
            tx.tx_ref().put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: keccak256(key), value: *value },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let tx =  factory.provider_rw().unwrap();
        reth_trie_db::with_adapter!(tx, |A| {
            let got = DbStorageRoot::<_, A>::from_tx(tx.tx_ref(), address).root().unwrap();
            let expected = storage_root(storage.into_iter());
            assert_eq!(expected, got);
        });
    });
}

#[test]
// This ensures we don't add empty accounts to the trie
#[allow(clippy::needless_update)]
fn test_empty_account() {
    let state: State = BTreeMap::from([
        (
            Address::random(),
            (
                Account { balance: U256::ZERO, ..Default::default() },
                BTreeMap::from([(B256::with_last_byte(0x4), U256::from(12))]),
            ),
        ),
        (
            Address::random(),
            (Account { balance: U256::ZERO, ..Default::default() }, BTreeMap::default()),
        ),
        (
            Address::random(),
            (
                Account {
                    nonce: 155,
                    balance: U256::from(414241124u32),
                    bytecode_hash: Some(keccak256("test")),
                    ..Default::default()
                },
                BTreeMap::from([(B256::ZERO, U256::from(3)), (B256::with_last_byte(2), U256::ONE)]),
            ),
        ),
    ]);
    test_state_root_with_state(state);
}

#[test]
// This ensures we return an empty root when there are no storage entries
#[allow(clippy::needless_update)]
fn test_empty_storage_root() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let address = Address::random();
    let code = "el buen fla";
    let account = Account {
        nonce: 155,
        balance: U256::from(414241124u32),
        bytecode_hash: Some(keccak256(code)),
        ..Default::default()
    };
    insert_account(tx.tx_ref(), address, account, &Default::default());
    tx.commit().unwrap();

    let tx = factory.provider_rw().unwrap();
    reth_trie_db::with_adapter!(tx, |A| {
        let (got, walked, updates) =
            DbStorageRoot::<_, A>::from_tx(tx.tx_ref(), address).root_with_updates().unwrap();
        assert_eq!(got, EMPTY_ROOT_HASH);
        assert_eq!(walked, 0);
        assert_eq!(updates, StorageTrieUpdates::default());
    });
}

#[test]
fn cleared_storage_emits_node_removals() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    let hashed_address = B256::random();
    let hashed_slots = [
        hex!("30af561000000000000000000000000000000000000000000000000000000000"),
        hex!("30af569000000000000000000000000000000000000000000000000000000000"),
        hex!("30af650000000000000000000000000000000000000000000000000000000000"),
        hex!("30af6f0000000000000000000000000000000000000000000000000000000000"),
        hex!("30af8f0000000000000000000000000000000000000000000000000000000000"),
        hex!("3100000000000000000000000000000000000000000000000000000000000000"),
    ]
    .map(B256::new);

    for hashed_slot in hashed_slots {
        tx.tx_ref()
            .put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: hashed_slot, value: U256::ONE },
            )
            .unwrap();
    }

    let (_, _, initial_updates) = reth_trie_db::with_adapter!(tx, |A| {
        DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address)
            .root_with_updates()
            .unwrap()
    });
    assert!(!initial_updates.storage_nodes_ref().is_empty());
    tx.write_storage_trie_updates_sorted(core::iter::once((
        &hashed_address,
        &initial_updates.into_sorted(),
    )))
    .unwrap();

    let mut hashed_storage_cursor =
        tx.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
    let mut prefix_set = PrefixSetMut::default();
    for hashed_slot in hashed_slots {
        let entry =
            hashed_storage_cursor.seek_by_key_subkey(hashed_address, hashed_slot).unwrap().unwrap();
        assert_eq!(entry.key, hashed_slot);
        hashed_storage_cursor.delete_current().unwrap();
        prefix_set.insert(Nibbles::unpack(hashed_slot));
    }
    drop(hashed_storage_cursor);

    reth_trie_db::with_adapter!(tx, |A| {
        let (root, _, updates) = DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address)
            .with_prefix_set(prefix_set.freeze())
            .root_with_updates()
            .unwrap();
        assert_eq!(root, EMPTY_ROOT_HASH);
        assert!(!updates.removed_nodes_ref().is_empty());
    });
}

#[test]
fn destroyed_account_storage_emits_node_removals() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    let hashed_address = B256::random();
    tx.tx_ref().put::<tables::HashedAccounts>(hashed_address, Account::default()).unwrap();
    for hashed_slot in [
        hex!("30af561000000000000000000000000000000000000000000000000000000000"),
        hex!("30af569000000000000000000000000000000000000000000000000000000000"),
        hex!("30af650000000000000000000000000000000000000000000000000000000000"),
        hex!("30af6f0000000000000000000000000000000000000000000000000000000000"),
        hex!("30af8f0000000000000000000000000000000000000000000000000000000000"),
        hex!("3100000000000000000000000000000000000000000000000000000000000000"),
    ] {
        tx.tx_ref()
            .put::<tables::HashedStorages>(
                hashed_address,
                StorageEntry { key: B256::new(hashed_slot), value: U256::ONE },
            )
            .unwrap();
    }

    let initial_updates = reth_trie_db::with_adapter!(tx, |A| {
        DbStateRoot::<_, A>::from_tx(tx.tx_ref()).root_with_updates().unwrap().1
    });
    tx.write_trie_updates(initial_updates).unwrap();

    let initial_storage_updates = reth_trie_db::with_adapter!(tx, |A| {
        DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address)
            .root_with_updates()
            .unwrap()
            .2
    });
    let mut expected_removed_nodes: Vec<_> =
        initial_storage_updates.storage_nodes_ref().keys().copied().collect();
    expected_removed_nodes.sort_unstable();
    assert!(!expected_removed_nodes.is_empty());
    let initial_storage_updates = initial_storage_updates.into_sorted();
    tx.write_storage_trie_updates_sorted(core::iter::once((
        &hashed_address,
        &initial_storage_updates,
    )))
    .unwrap();

    let mut account_cursor = tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();
    account_cursor.seek_exact(hashed_address).unwrap();
    account_cursor.delete_current().unwrap();
    drop(account_cursor);

    let mut storage_cursor = tx.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
    storage_cursor.seek_exact(hashed_address).unwrap();
    storage_cursor.delete_current_duplicates().unwrap();
    drop(storage_cursor);

    let mut account_prefix_set = PrefixSetMut::default();
    account_prefix_set.insert(Nibbles::unpack(hashed_address));
    let (root, updates) = reth_trie_db::with_adapter!(tx, |A| {
        DbStateRoot::<_, A>::from_tx(tx.tx_ref())
            .with_prefix_sets(TriePrefixSets {
                account_prefix_set: account_prefix_set.freeze(),
                destroyed_accounts: B256Set::from_iter([hashed_address]),
                ..Default::default()
            })
            .root_with_updates()
            .unwrap()
    });

    assert_eq!(root, EMPTY_ROOT_HASH);
    let storage_updates = &updates.storage_tries_ref()[&hashed_address];
    let mut removed_nodes: Vec<_> = storage_updates.removed_nodes_ref().iter().copied().collect();
    removed_nodes.sort_unstable();
    assert_eq!(removed_nodes, expected_removed_nodes);

    tx.write_trie_updates(updates).unwrap();
    assert!(tx
        .tx_ref()
        .cursor_dup_read::<tables::StoragesTrie>()
        .unwrap()
        .seek_exact(hashed_address)
        .unwrap()
        .is_none());
}

#[test]
// This ensures that the walker goes over all the storage slots
#[allow(clippy::needless_update)]
fn test_storage_root() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let address = Address::random();
    let storage =
        BTreeMap::from([(B256::ZERO, U256::from(3)), (B256::with_last_byte(2), U256::ONE)]);

    let code = "el buen fla";
    let account = Account {
        nonce: 155,
        balance: U256::from(414241124u32),
        bytecode_hash: Some(keccak256(code)),
        ..Default::default()
    };

    insert_account(tx.tx_ref(), address, account, &storage);
    tx.commit().unwrap();

    let tx = factory.provider_rw().unwrap();
    reth_trie_db::with_adapter!(tx, |A| {
        let got = DbStorageRoot::<_, A>::from_tx(tx.tx_ref(), address).root().unwrap();
        assert_eq!(storage_root(storage.into_iter()), got);
    });
}

type State = BTreeMap<Address, (Account, BTreeMap<B256, U256>)>;

#[test]
fn arbitrary_state_root() {
    proptest!(
        ProptestConfig::with_cases(10), | (state in arb::<State>()) | {
            test_state_root_with_state(state);
        }
    );
}

#[test]
fn arbitrary_state_root_with_progress() {
    proptest!(
        ProptestConfig::with_cases(10), | (state in arb::<State>()) | {
            let hashed_entries_total = state.len() +
                state.values().map(|(_, slots)| slots.len()).sum::<usize>();

            let factory = create_test_provider_factory();
            let tx = factory.provider_rw().unwrap();

            for (address, (account, storage)) in &state {
                insert_account(tx.tx_ref(), *address, account.clone(), storage)
            }
            tx.commit().unwrap();
            let tx =  factory.provider_rw().unwrap();

            let expected = state_root(state);

            let threshold = 10;
            let mut got = None;
            let mut hashed_entries_walked = 0;

            let mut intermediate_state: Option<Box<IntermediateStateRootState>> = None;
            reth_trie_db::with_adapter!(tx, |A| {
                while got.is_none() {
                    let calculator = DbStateRoot::<_, A>::from_tx(tx.tx_ref())
                        .with_threshold(threshold)
                        .with_intermediate_state(intermediate_state.take().map(|state| *state));
                    match calculator.root_with_progress().unwrap() {
                        StateRootProgress::Progress(state, walked, _) => {
                            intermediate_state = Some(state);
                            hashed_entries_walked += walked;
                        },
                        StateRootProgress::Complete(root, walked, _) => {
                            got = Some(root);
                            hashed_entries_walked += walked;
                        },
                    };
                }
            });
            assert_eq!(expected, got.unwrap());
            assert_eq!(hashed_entries_total, hashed_entries_walked)
        }
    );
}

fn test_state_root_with_state(state: State) {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    for (address, (account, storage)) in &state {
        insert_account(tx.tx_ref(), *address, account.clone(), storage)
    }
    tx.commit().unwrap();
    let expected = state_root(state);

    let tx = factory.provider_rw().unwrap();
    reth_trie_db::with_adapter!(tx, |A| {
        let got = DbStateRoot::<_, A>::from_tx(tx.tx_ref()).root().unwrap();
        assert_eq!(expected, got);
    });
}

fn encode_account(account: &Account, storage_root: Option<B256>) -> Vec<u8> {
    let account = account.clone().into_trie_account(storage_root.unwrap_or(EMPTY_ROOT_HASH));
    let mut account_rlp = Vec::with_capacity(account.length());
    account.encode(&mut account_rlp);
    account_rlp
}

#[test]
fn storage_root_regression() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    // Some address whose hash starts with 0xB041
    let address3 = address!("0x16b07afd1c635f77172e842a000ead9a2a222459");
    let key3 = keccak256(address3);
    assert_eq!(key3[0], 0xB0);
    assert_eq!(key3[1], 0x41);

    let storage = BTreeMap::from(
        [
            ("1200000000000000000000000000000000000000000000000000000000000000", 0x42),
            ("1400000000000000000000000000000000000000000000000000000000000000", 0x01),
            ("3000000000000000000000000000000000000000000000000000000000E00000", 0x127a89),
            ("3000000000000000000000000000000000000000000000000000000000E00001", 0x05),
        ]
        .map(|(slot, val)| (B256::from_str(slot).unwrap(), U256::from(val))),
    );

    let mut hashed_storage_cursor =
        tx.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();
    for (hashed_slot, value) in storage.clone() {
        hashed_storage_cursor.upsert(key3, &StorageEntry { key: hashed_slot, value }).unwrap();
    }
    tx.commit().unwrap();
    let tx = factory.provider_rw().unwrap();

    reth_trie_db::with_adapter!(tx, |A| {
        let account3_storage_root =
            DbStorageRoot::<_, A>::from_tx(tx.tx_ref(), address3).root().unwrap();
        let expected_root = storage_root_prehashed(storage);
        assert_eq!(expected_root, account3_storage_root);
    });
}

#[test]
fn account_and_storage_trie() {
    let ether = U256::from(1e18);
    let storage = BTreeMap::from(
        [
            ("1200000000000000000000000000000000000000000000000000000000000000", 0x42),
            ("1400000000000000000000000000000000000000000000000000000000000000", 0x01),
            ("3000000000000000000000000000000000000000000000000000000000E00000", 0x127a89),
            ("3000000000000000000000000000000000000000000000000000000000E00001", 0x05),
        ]
        .map(|(slot, val)| (B256::from_str(slot).unwrap(), U256::from(val))),
    );

    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let mut hashed_account_cursor = tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();
    let mut hashed_storage_cursor =
        tx.tx_ref().cursor_dup_write::<tables::HashedStorages>().unwrap();

    let mut hash_builder = HashBuilder::default();

    // Insert first account
    let key1 = b256!("0xb000000000000000000000000000000000000000000000000000000000000000");
    let account1 = Account { balance: U256::from(3).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key1, &account1).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key1), &encode_account(&account1, None));

    // Some address whose hash starts with 0xB040
    let address2 = address!("0x7db3e81b72d2695e19764583f6d219dbee0f35ca");
    let key2 = keccak256(address2);
    assert_eq!(key2[0], 0xB0);
    assert_eq!(key2[1], 0x40);
    let account2 = Account { balance: ether, ..Default::default() };
    hashed_account_cursor.upsert(key2, &account2).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key2), &encode_account(&account2, None));

    // Some address whose hash starts with 0xB041
    let address3 = address!("0x16b07afd1c635f77172e842a000ead9a2a222459");
    let key3 = keccak256(address3);
    assert_eq!(key3[0], 0xB0);
    assert_eq!(key3[1], 0x41);
    let code_hash = b256!("0x5be74cad16203c4905c068b012a2e9fb6d19d036c410f16fd177f337541440dd");
    let account3 = Account {
        balance: U256::from(2).mul(ether),
        bytecode_hash: Some(code_hash),
        ..Default::default()
    };
    hashed_account_cursor.upsert(key3, &account3).unwrap();
    for (hashed_slot, value) in storage {
        if hashed_storage_cursor
            .seek_by_key_subkey(key3, hashed_slot)
            .unwrap()
            .is_some_and(|e| e.key == hashed_slot)
        {
            hashed_storage_cursor.delete_current().unwrap();
        }
        hashed_storage_cursor.upsert(key3, &StorageEntry { key: hashed_slot, value }).unwrap();
    }
    let account3_storage_root = reth_trie_db::with_adapter!(tx, |A| {
        DbStorageRoot::<_, A>::from_tx(tx.tx_ref(), address3).root().unwrap()
    });
    hash_builder
        .add_leaf(Nibbles::unpack(key3), &encode_account(&account3, Some(account3_storage_root)));

    let key4a = b256!("0xB1A0000000000000000000000000000000000000000000000000000000000000");
    let account4a = Account { balance: U256::from(4).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key4a, &account4a).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key4a), &encode_account(&account4a, None));

    let key5 = b256!("0xB310000000000000000000000000000000000000000000000000000000000000");
    let account5 = Account { balance: U256::from(8).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key5, &account5).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key5), &encode_account(&account5, None));

    let key6 = b256!("0xB340000000000000000000000000000000000000000000000000000000000000");
    let account6 = Account { balance: U256::ONE.mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key6, &account6).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key6), &encode_account(&account6, None));

    // Populate account & storage trie DB tables
    let expected_root = b256!("0x72861041bc90cd2f93777956f058a545412b56de79af5eb6b8075fe2eabbe015");
    let computed_expected_root: B256 = triehash::trie_root::<KeccakHasher, _, _, _>([
        (key1, encode_account(&account1, None)),
        (key2, encode_account(&account2, None)),
        (key3, encode_account(&account3, Some(account3_storage_root))),
        (key4a, encode_account(&account4a, None)),
        (key5, encode_account(&account5, None)),
        (key6, encode_account(&account6, None)),
    ]);
    // Check computed trie root to ensure correctness
    assert_eq!(computed_expected_root, expected_root);

    // Check hash builder root
    assert_eq!(hash_builder.root(), computed_expected_root);

    // Check state root calculation from scratch
    let (root, trie_updates) = reth_trie_db::with_adapter!(tx, |A| {
        DbStateRoot::<_, A>::from_tx(tx.tx_ref()).root_with_updates().unwrap()
    });
    assert_eq!(root, computed_expected_root);

    // Check account trie
    let account_updates = trie_updates.into_sorted();
    let account_updates = account_updates.account_nodes_ref();
    assert_eq!(account_updates.len(), 2);

    let (nibbles1a, node1a) = account_updates.first().unwrap();
    assert_eq!(nibbles1a.to_vec(), vec![0xB]);
    let node1a = node1a.as_ref().unwrap();
    assert_eq!(node1a.state_mask, TrieMask::new(0b1011));
    assert_eq!(node1a.tree_mask, TrieMask::new(0b0001));
    assert_eq!(node1a.hash_mask, TrieMask::new(0b1001));
    assert_eq!(node1a.root_hash, None);
    assert_eq!(node1a.hashes.len(), 2);

    let (nibbles2a, node2a) = account_updates.last().unwrap();
    assert_eq!(nibbles2a.to_vec(), vec![0xB, 0x0]);
    let node2a = node2a.as_ref().unwrap();
    assert_eq!(node2a.state_mask, TrieMask::new(0b10001));
    assert_eq!(node2a.tree_mask, TrieMask::new(0b00000));
    assert_eq!(node2a.hash_mask, TrieMask::new(0b10000));
    assert_eq!(node2a.root_hash, None);
    assert_eq!(node2a.hashes.len(), 1);

    // Add an account
    // Some address whose hash starts with 0xB1
    let address4b = address!("0x4f61f2d5ebd991b85aa1677db97307caf5215c91");
    let key4b = keccak256(address4b);
    assert_eq!(key4b.0[0], key4a.0[0]);
    let account4b = Account { balance: U256::from(5).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key4b, &account4b).unwrap();

    let mut prefix_set = PrefixSetMut::default();
    prefix_set.insert(Nibbles::unpack(key4b));

    let expected_state_root =
        b256!("0x8e263cd4eefb0c3cbbb14e5541a66a755cad25bcfab1e10dd9d706263e811b28");

    let (root, trie_updates) = reth_trie_db::with_adapter!(tx, |A| {
        DbStateRoot::<_, A>::from_tx(tx.tx_ref())
            .with_prefix_sets(TriePrefixSets {
                account_prefix_set: prefix_set.freeze(),
                ..Default::default()
            })
            .root_with_updates()
            .unwrap()
    });
    assert_eq!(root, expected_state_root);

    let account_updates = trie_updates.into_sorted();
    let account_updates = account_updates.account_nodes_ref();
    assert_eq!(account_updates.len(), 2);

    let (nibbles1b, node1b) = account_updates.first().unwrap();
    assert_eq!(nibbles1b.to_vec(), vec![0xB]);
    let node1b = node1b.as_ref().unwrap();
    assert_eq!(node1b.state_mask, TrieMask::new(0b1011));
    assert_eq!(node1b.tree_mask, TrieMask::new(0b0001));
    assert_eq!(node1b.hash_mask, TrieMask::new(0b1011));
    assert_eq!(node1b.root_hash, None);
    assert_eq!(node1b.hashes.len(), 3);
    assert_eq!(node1a.hashes[0], node1b.hashes[0]);
    assert_eq!(node1a.hashes[1], node1b.hashes[2]);

    let (nibbles2b, node2b) = account_updates.last().unwrap();
    assert_eq!(nibbles2b.to_vec(), vec![0xB, 0x0]);
    let node2b = node2b.as_ref().unwrap();
    assert_eq!(node2a, node2b);
    tx.commit().unwrap();

    {
        let tx = factory.provider_rw().unwrap();
        let mut hashed_account_cursor =
            tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        let account = hashed_account_cursor.seek_exact(key2).unwrap().unwrap();
        hashed_account_cursor.delete_current().unwrap();

        let mut account_prefix_set = PrefixSetMut::default();
        account_prefix_set.insert(Nibbles::unpack(account.0));

        let computed_expected_root: B256 = triehash::trie_root::<KeccakHasher, _, _, _>([
            (key1, encode_account(&account1, None)),
            // DELETED: (key2, encode_account(&account2, None)),
            (key3, encode_account(&account3, Some(account3_storage_root))),
            (key4a, encode_account(&account4a, None)),
            (key4b, encode_account(&account4b, None)),
            (key5, encode_account(&account5, None)),
            (key6, encode_account(&account6, None)),
        ]);

        let (root, trie_updates) = reth_trie_db::with_adapter!(tx, |A| {
            DbStateRoot::<_, A>::from_tx(tx.tx_ref())
                .with_prefix_sets(TriePrefixSets {
                    account_prefix_set: account_prefix_set.freeze(),
                    ..Default::default()
                })
                .root_with_updates()
                .unwrap()
        });
        assert_eq!(root, computed_expected_root);
        assert_eq!(
            trie_updates.account_nodes_ref().len() + trie_updates.removed_nodes_ref().len(),
            1
        );

        assert_eq!(trie_updates.account_nodes_ref().len(), 1);

        let entry = trie_updates.account_nodes_ref().iter().next().unwrap();
        assert_eq!(entry.0.to_vec(), vec![0xB]);
        let node1c = entry.1;

        assert_eq!(node1c.state_mask, TrieMask::new(0b1011));
        assert_eq!(node1c.tree_mask, TrieMask::new(0b0000));
        assert_eq!(node1c.hash_mask, TrieMask::new(0b1011));

        assert_eq!(node1c.root_hash, None);

        assert_eq!(node1c.hashes.len(), 3);
        assert_ne!(node1c.hashes[0], node1b.hashes[0]);
        assert_eq!(node1c.hashes[1], node1b.hashes[1]);
        assert_eq!(node1c.hashes[2], node1b.hashes[2]);
    }

    {
        let tx = factory.provider_rw().unwrap();
        let mut hashed_account_cursor =
            tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        let account2 = hashed_account_cursor.seek_exact(key2).unwrap().unwrap();
        hashed_account_cursor.delete_current().unwrap();
        let account3 = hashed_account_cursor.seek_exact(key3).unwrap().unwrap();
        hashed_account_cursor.delete_current().unwrap();

        let mut account_prefix_set = PrefixSetMut::default();
        account_prefix_set.insert(Nibbles::unpack(account2.0));
        account_prefix_set.insert(Nibbles::unpack(account3.0));

        let computed_expected_root: B256 = triehash::trie_root::<KeccakHasher, _, _, _>([
            (key1, encode_account(&account1, None)),
            // DELETED: (key2, encode_account(&account2, None)),
            // DELETED: (key3, encode_account(&account3, Some(account3_storage_root))),
            (key4a, encode_account(&account4a, None)),
            (key4b, encode_account(&account4b, None)),
            (key5, encode_account(&account5, None)),
            (key6, encode_account(&account6, None)),
        ]);

        let (root, trie_updates) = reth_trie_db::with_adapter!(tx, |A| {
            DbStateRoot::<_, A>::from_tx(tx.tx_ref())
                .with_prefix_sets(TriePrefixSets {
                    account_prefix_set: account_prefix_set.freeze(),
                    ..Default::default()
                })
                .root_with_updates()
                .unwrap()
        });
        assert_eq!(root, computed_expected_root);
        assert_eq!(
            trie_updates.account_nodes_ref().len() + trie_updates.removed_nodes_ref().len(),
            1
        );
        assert!(!trie_updates
            .storage_tries_ref()
            .iter()
            .any(|(_, u)| !u.storage_nodes_ref().is_empty() || !u.removed_nodes_ref().is_empty())); // no storage root update

        assert_eq!(trie_updates.account_nodes_ref().len(), 1);

        let entry = trie_updates.account_nodes_ref().iter().next().unwrap();
        assert_eq!(entry.0.to_vec(), vec![0xB]);
        let node1d = entry.1;

        assert_eq!(node1d.state_mask, TrieMask::new(0b1011));
        assert_eq!(node1d.tree_mask, TrieMask::new(0b0000));
        assert_eq!(node1d.hash_mask, TrieMask::new(0b1010));

        assert_eq!(node1d.root_hash, None);

        assert_eq!(node1d.hashes.len(), 2);
        assert_eq!(node1d.hashes[0], node1b.hashes[1]);
        assert_eq!(node1d.hashes[1], node1b.hashes[2]);
    }
}

#[test]
fn account_trie_around_extension_node() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let expected = extension_node_trie(&tx);

    let (got, updates) = reth_trie_db::with_adapter!(tx, |A| {
        DbStateRoot::<_, A>::from_tx(tx.tx_ref()).root_with_updates().unwrap()
    });
    assert_eq!(expected, got);
    assert_trie_updates(updates.account_nodes_ref());
}

#[test]
fn account_trie_around_extension_node_with_dbtrie() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let expected = extension_node_trie(&tx);

    let (got, updates) = reth_trie_db::with_adapter!(tx, |A| {
        DbStateRoot::<_, A>::from_tx(tx.tx_ref()).root_with_updates().unwrap()
    });
    assert_eq!(expected, got);
    tx.write_trie_updates(updates).unwrap();

    // read the account updates from the db
    let mut accounts_trie = tx.tx_ref().cursor_read::<tables::AccountsTrie>().unwrap();
    let walker = accounts_trie.walk(None).unwrap();
    let account_updates = walker
        .into_iter()
        .map(|item| {
            let (key, node) = item.unwrap();
            (key.0, node)
        })
        .collect();
    assert_trie_updates(&account_updates);
}

#[test]
fn fuzz_state_root_incremental() {
    // The cases share one factory: each drops its provider without committing, so the next case
    // starts from an empty database.
    let factory = create_test_provider_factory();
    proptest!(ProptestConfig::with_cases(128), |(
        account_changes in any::<[BTreeMap<B256, U256>; 5]>(),
    )| {
        let tx = factory.provider_rw().unwrap();
        let mut hashed_account_cursor = tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        let mut state = BTreeMap::default();
        for mut accounts in account_changes {
            let should_generate_changeset = !state.is_empty();
            let mut changes = PrefixSetMut::default();
            for (hashed_address, balance) in accounts.clone() {
                hashed_account_cursor.upsert(hashed_address, &Account { balance, ..Default::default() }).unwrap();
                if should_generate_changeset {
                    changes.insert(Nibbles::unpack(hashed_address));
                }
            }

            let (state_root, trie_updates) = reth_trie_db::with_adapter!(tx, |A| {
                DbStateRoot::<_, A>::from_tx(tx.tx_ref())
                    .with_prefix_sets(TriePrefixSets { account_prefix_set: changes.freeze(), ..Default::default() })
                    .root_with_updates()
                    .unwrap()
            });

            state.append(&mut accounts);
            let expected_root = state_root_prehashed(
                state.iter().map(|(&key, &balance)| (key, (Account { balance, ..Default::default() }, std::iter::empty())))
            );
            assert_eq!(expected_root, state_root);
            tx.write_trie_updates(trie_updates).unwrap();
        }
    });
}

#[test]
fn storage_trie_around_extension_node() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let hashed_address = B256::random();
    let (expected_root, expected_updates) = extension_node_storage_trie(&tx, hashed_address);

    reth_trie_db::with_adapter!(tx, |A| {
        let (got, _, updates) = DbStorageRoot::<_, A>::from_tx_hashed(tx.tx_ref(), hashed_address)
            .root_with_updates()
            .unwrap();
        assert_eq!(expected_root, got);
        assert_eq!(expected_updates, updates);
        assert_trie_updates(updates.storage_nodes_ref());
    });
}

fn extension_node_storage_trie<N: ProviderNodeTypes>(
    tx: &DatabaseProviderRW<Arc<TempDatabase<DatabaseEnv>>, N>,
    hashed_address: B256,
) -> (B256, StorageTrieUpdates) {
    let value = U256::ONE;

    let mut hashed_storage = tx.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();

    let mut hb = HashBuilder::default().with_updates(true);

    for key in [
        hex!("30af561000000000000000000000000000000000000000000000000000000000"),
        hex!("30af569000000000000000000000000000000000000000000000000000000000"),
        hex!("30af650000000000000000000000000000000000000000000000000000000000"),
        hex!("30af6f0000000000000000000000000000000000000000000000000000000000"),
        hex!("30af8f0000000000000000000000000000000000000000000000000000000000"),
        hex!("3100000000000000000000000000000000000000000000000000000000000000"),
    ] {
        hashed_storage
            .upsert(hashed_address, &StorageEntry { key: B256::new(key), value })
            .unwrap();
        hb.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode_fixed_size(&value));
    }

    let root = hb.root();
    let (_, updates) = hb.split();
    let trie_updates = StorageTrieUpdates::new(updates);
    (root, trie_updates)
}

fn extension_node_trie<N: ProviderNodeTypes>(
    tx: &DatabaseProviderRW<Arc<TempDatabase<DatabaseEnv>>, N>,
) -> B256 {
    let a =
        Account { balance: U256::ONE, bytecode_hash: Some(B256::random()), ..Default::default() };
    let val = encode_account(&a, None);

    let mut hashed_accounts = tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();
    let mut hb = HashBuilder::default();

    for key in [
        hex!("30af561000000000000000000000000000000000000000000000000000000000"),
        hex!("30af569000000000000000000000000000000000000000000000000000000000"),
        hex!("30af650000000000000000000000000000000000000000000000000000000000"),
        hex!("30af6f0000000000000000000000000000000000000000000000000000000000"),
        hex!("30af8f0000000000000000000000000000000000000000000000000000000000"),
        hex!("3100000000000000000000000000000000000000000000000000000000000000"),
    ] {
        hashed_accounts.upsert(B256::new(key), &a).unwrap();
        hb.add_leaf(Nibbles::unpack(key), &val);
    }

    hb.root()
}

fn assert_trie_updates(account_updates: &HashMap<Nibbles, BranchNodeCompact>) {
    assert_eq!(account_updates.len(), 2);

    let node = account_updates.get(&Nibbles::from_nibbles_unchecked([0x3])).unwrap();
    let expected = BranchNodeCompact::new(0b0011, 0b0001, 0b0000, vec![], None);
    assert_eq!(node, &expected);

    let node = account_updates.get(&Nibbles::from_nibbles_unchecked([0x3, 0x0, 0xA, 0xF])).unwrap();
    assert_eq!(node.state_mask, TrieMask::new(0b101100000));
    assert_eq!(node.tree_mask, TrieMask::new(0b000000000));
    assert_eq!(node.hash_mask, TrieMask::new(0b001000000));

    assert_eq!(node.root_hash, None);
    assert_eq!(node.hashes.len(), 1);
}

/// Hashed state model: account and non-zero storage slots by hashed address.
type HashedState = BTreeMap<B256, (Account, BTreeMap<B256, U256>)>;

/// A slot change: an existing slot picked by index, or the fresh slot, and its new value.
type SlotChange = (Index, bool, B256, U256);

/// An account change: an existing account picked by index, or the fresh address, and its new
/// value with slot changes, or `None` to destroy it.
type AccountChange = (Index, bool, B256, Option<(Account, Vec<SlotChange>)>);

/// Account and storage trie nodes of one adapter's tables, in table order.
type StoredTrieNodes = (Vec<(Nibbles, BranchNodeCompact)>, Vec<(B256, Nibbles, BranchNodeCompact)>);

/// Checks that the legacy, packed and memcmp trie key adapters compute the same full and
/// incremental state roots, trie updates and stored trie nodes over several rounds of changes.
#[test]
fn incremental_root_matches_across_trie_key_adapters() {
    // The adapters view the same trie tables, so each gets its own database. The cases share
    // them: each drops its provider without committing.
    let legacy_factory = create_test_provider_factory();
    let packed_factory = create_test_provider_factory();
    let memcmp_factory = create_test_provider_factory();
    let max_path_len = Cell::new(0);
    proptest!(ProptestConfig::with_cases(64), |(
        initial in collection::vec(account_change(), 1..48),
        rounds in collection::vec(collection::vec(account_change(), 0..16), 1..4),
    )| {
        let mut state = HashedState::default();
        let mut post_states = Vec::new();
        let mut expected_roots = Vec::new();
        for changes in std::iter::once(initial).chain(rounds) {
            post_states.push(apply_account_changes(&mut state, changes));
            let accounts = state.iter().map(|(address, (account, storage))| {
                (*address, (account.clone(), storage.iter().map(|(slot, value)| (*slot, *value))))
            });
            expected_roots.push(state_root_prehashed(accounts));
        }

        let legacy = roots_with_adapter::<LegacyKeyAdapter>(&legacy_factory, &post_states);
        let packed = roots_with_adapter::<PackedKeyAdapter>(&packed_factory, &post_states);
        let memcmp = roots_with_adapter::<MemcmpKeyAdapter>(&memcmp_factory, &post_states);

        let roots = legacy.0.iter().map(|(root, _)| *root).collect::<Vec<_>>();
        assert_eq!(roots, expected_roots);
        assert_eq!(packed, legacy);
        assert_eq!(memcmp, legacy);

        let (account_nodes, storage_nodes) = &memcmp.1;
        let path_lens = account_nodes
            .iter()
            .map(|(path, _)| path.len())
            .chain(storage_nodes.iter().map(|(_, path, _)| path.len()));
        max_path_len.set(path_lens.fold(max_path_len.get(), usize::max));
    });

    // Paths over 16 nibbles take at least three memcmp groups.
    assert!(max_path_len.get() > 16, "deepest stored path has {} nibbles", max_path_len.get());
}

/// Returns hashed keys sharing up to 12 leading bytes with one of three fixed keys, so the tries
/// get branch nodes deeper than one memcmp group.
///
/// One fixed key is all zeros, so stored paths often end in zero nibbles, which the memcmp
/// encoding also uses as padding.
fn clustered_hashed_key() -> impl Strategy<Value = B256> {
    (0u8..3, 0usize..=12, any::<B256>()).prop_map(|(cluster, shared, mut key)| {
        let base = if cluster == 0 { [0u8; 32] } else { keccak256([cluster]).0 };
        key.0[..shared].copy_from_slice(&base[..shared]);
        key
    })
}

fn account_change() -> impl Strategy<Value = AccountChange> {
    let slot_change = (
        any::<Index>(),
        any::<bool>(),
        clustered_hashed_key(),
        prop_oneof![Just(U256::ZERO), (1..=u64::MAX).prop_map(U256::from)],
    );
    (
        any::<Index>(),
        any::<bool>(),
        clustered_hashed_key(),
        option::weighted(0.8, (arb::<Account>(), collection::vec(slot_change, 0..12))),
    )
}

/// Applies `changes` to `state` and returns them as a [`HashedPostState`].
///
/// The slots of a destroyed account are zeroed in the post state, so writing it clears them.
fn apply_account_changes(state: &mut HashedState, changes: Vec<AccountChange>) -> HashedPostState {
    let mut post_state = HashedPostState::default();
    for (index, fresh, fresh_address, change) in changes {
        let hashed_address = if fresh || state.is_empty() {
            fresh_address
        } else {
            *state.keys().nth(index.index(state.len())).unwrap()
        };
        let hashed_storage = post_state.storages.entry(hashed_address).or_default();
        match change {
            Some((account, slot_changes)) => {
                let (current, storage) = state.entry(hashed_address).or_default();
                *current = account.clone();
                for (index, fresh, fresh_slot, value) in slot_changes {
                    let slot = if fresh || storage.is_empty() {
                        fresh_slot
                    } else {
                        *storage.keys().nth(index.index(storage.len())).unwrap()
                    };
                    if value.is_zero() {
                        storage.remove(&slot);
                    } else {
                        storage.insert(slot, value);
                    }
                    hashed_storage.storage.insert(slot, value);
                }
                post_state.accounts.insert(hashed_address, Some(account));
            }
            None => {
                if let Some((_, storage)) = state.remove(&hashed_address) {
                    hashed_storage
                        .storage
                        .extend(storage.into_keys().map(|slot| (slot, U256::ZERO)));
                }
                post_state.accounts.insert(hashed_address, None);
            }
        }
    }
    post_state
}

/// Writes each post state and computes the state root through adapter `A`, writing the trie
/// updates through `A` before the next round.
///
/// The first root is a full computation, later roots are incremental over the stored trie.
fn roots_with_adapter<A: TrieTableAdapter>(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    post_states: &[HashedPostState],
) -> (Vec<(B256, TrieUpdatesSorted)>, StoredTrieNodes) {
    let tx = factory.provider_rw().unwrap();
    let mut results = Vec::new();
    for (round, post_state) in post_states.iter().enumerate() {
        tx.write_hashed_state(&post_state.clone_into_sorted()).unwrap();

        let calculator = DbStateRoot::<_, A>::from_tx(tx.tx_ref());
        let calculator = if round == 0 {
            calculator
        } else {
            calculator.with_prefix_sets(post_state.construct_prefix_sets().freeze())
        };
        let (root, updates) = calculator.root_with_updates().unwrap();
        let updates = updates.into_sorted();
        write_trie_updates_with_adapter::<A>(tx.tx_ref(), &updates);
        results.push((root, updates));
    }
    (results, stored_trie_nodes::<A>(tx.tx_ref()))
}

/// Writes `updates` to the trie tables of adapter `A`, like [`TrieWriter::write_trie_updates`].
fn write_trie_updates_with_adapter<A: TrieTableAdapter>(
    tx: &(impl DbTx + DbTxMut),
    updates: &TrieUpdatesSorted,
) {
    let mut account_cursor = tx.cursor_write::<A::AccountTrieTable>().unwrap();
    for (path, node) in updates.account_nodes_ref() {
        match node {
            Some(node) if !path.is_empty() => account_cursor.upsert((*path).into(), node).unwrap(),
            Some(_) => {}
            None => {
                if account_cursor.seek_exact((*path).into()).unwrap().is_some() {
                    account_cursor.delete_current().unwrap();
                }
            }
        }
    }

    for (hashed_address, storage_updates) in updates.storage_tries_ref() {
        let cursor = tx.cursor_dup_write::<A::StorageTrieTable>().unwrap();
        DatabaseStorageTrieCursor::<_, A>::new(cursor, *hashed_address)
            .write_storage_trie_updates_sorted(storage_updates)
            .unwrap();
    }
}

fn stored_trie_nodes<A: TrieTableAdapter>(tx: &impl DbTx) -> StoredTrieNodes {
    let account_nodes = tx
        .cursor_read::<A::AccountTrieTable>()
        .unwrap()
        .walk(None)
        .unwrap()
        .map(|entry| {
            let (key, node) = entry.unwrap();
            (A::account_key_to_nibbles(&key), node)
        })
        .collect();
    let storage_nodes = tx
        .cursor_dup_read::<A::StorageTrieTable>()
        .unwrap()
        .walk(None)
        .unwrap()
        .map(|entry| {
            let (hashed_address, entry) = entry.unwrap();
            let (subkey, node) = entry.into_parts();
            (hashed_address, A::subkey_to_nibbles(&subkey), node)
        })
        .collect();
    (account_nodes, storage_nodes)
}
