#![allow(missing_docs)]

use alloy_primitives::{hex_literal::hex, keccak256, Address, B256, U256};
use proptest::{prelude::ProptestConfig, proptest};
use proptest_arbitrary_interop::arb;
use reth_db::{tables, test_utils::TempDatabase, DatabaseEnv};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    transaction::DbTxMut,
};
use reth_primitives::{constants::EMPTY_ROOT_HASH, Account, StorageEntry};
use reth_provider::{
    test_utils::create_test_provider_factory, DatabaseProviderRW, StorageTrieWriter, TrieWriter,
};
use reth_trie::{
    prefix_set::PrefixSetMut,
    test_utils::{state_root, state_root_prehashed, storage_root, storage_root_prehashed},
    BranchNodeCompact, StateRoot, StorageRoot, TrieMask,
};
use reth_trie_common::triehash::KeccakHasher;
use reth_trie_db::{DatabaseStateRoot, DatabaseStorageRoot};
use std::{
    collections::{BTreeMap, HashMap},
    ops::Mul,
    str::FromStr,
    sync::Arc,
};

use alloy_rlp::Encodable;
use reth_db_api::transaction::DbTx;
use reth_trie::{
    prefix_set::TriePrefixSets, updates::StorageTrieUpdates, HashBuilder,
    IntermediateStateRootState, Nibbles, StateRootProgress, TrieAccount,
};

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
    let value = U256::from(0);
    for key in data {
        hashed_storage_cursor.upsert(hashed_address, StorageEntry { key, value }).unwrap();
    }

    // Generate the intermediate nodes on the receiving end of the channel
    let (_, _, trie_updates) =
        StorageRoot::from_tx_hashed(tx.tx_ref(), hashed_address).root_with_updates().unwrap();

    // 1. Some state transition happens, update the hashed storage to the new value
    let modified_key = B256::from_str(modified).unwrap();
    let value = U256::from(1);
    if hashed_storage_cursor.seek_by_key_subkey(hashed_address, modified_key).unwrap().is_some() {
        hashed_storage_cursor.delete_current().unwrap();
    }
    hashed_storage_cursor
        .upsert(hashed_address, StorageEntry { key: modified_key, value })
        .unwrap();

    // 2. Calculate full merkle root
    let loader = StorageRoot::from_tx_hashed(tx.tx_ref(), hashed_address);
    let modified_root = loader.root().unwrap();

    // Update the intermediate roots table so that we can run the incremental verification
    tx.write_individual_storage_trie_updates(hashed_address, &trie_updates).unwrap();

    // 3. Calculate the incremental root
    let mut storage_changes = PrefixSetMut::default();
    storage_changes.insert(Nibbles::unpack(modified_key));
    let loader = StorageRoot::from_tx_hashed(tx.tx_ref(), hashed_address)
        .with_prefix_set(storage_changes.freeze());
    let incremental_root = loader.root().unwrap();

    assert_eq!(modified_root, incremental_root);
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
        let got = StorageRoot::from_tx(tx.tx_ref(), address).root().unwrap();
        let expected = storage_root(storage.into_iter());
        assert_eq!(expected, got);
    });
}

#[test]
// This ensures we dont add empty accounts to the trie
fn test_empty_account() {
    let state: State = BTreeMap::from([
        (
            Address::random(),
            (
                Account { nonce: 0, balance: U256::from(0), bytecode_hash: None },
                BTreeMap::from([(B256::with_last_byte(0x4), U256::from(12))]),
            ),
        ),
        (
            Address::random(),
            (
                Account { nonce: 0, balance: U256::from(0), bytecode_hash: None },
                BTreeMap::default(),
            ),
        ),
        (
            Address::random(),
            (
                Account {
                    nonce: 155,
                    balance: U256::from(414241124u32),
                    bytecode_hash: Some(keccak256("test")),
                },
                BTreeMap::from([
                    (B256::ZERO, U256::from(3)),
                    (B256::with_last_byte(2), U256::from(1)),
                ]),
            ),
        ),
    ]);
    test_state_root_with_state(state);
}

#[test]
// This ensures we return an empty root when there are no storage entries
fn test_empty_storage_root() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let address = Address::random();
    let code = "el buen fla";
    let account = Account {
        nonce: 155,
        balance: U256::from(414241124u32),
        bytecode_hash: Some(keccak256(code)),
    };
    insert_account(tx.tx_ref(), address, account, &Default::default());
    tx.commit().unwrap();

    let tx = factory.provider_rw().unwrap();
    let got = StorageRoot::from_tx(tx.tx_ref(), address).root().unwrap();
    assert_eq!(got, EMPTY_ROOT_HASH);
}

#[test]
// This ensures that the walker goes over all the storage slots
fn test_storage_root() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let address = Address::random();
    let storage =
        BTreeMap::from([(B256::ZERO, U256::from(3)), (B256::with_last_byte(2), U256::from(1))]);

    let code = "el buen fla";
    let account = Account {
        nonce: 155,
        balance: U256::from(414241124u32),
        bytecode_hash: Some(keccak256(code)),
    };

    insert_account(tx.tx_ref(), address, account, &storage);
    tx.commit().unwrap();

    let tx = factory.provider_rw().unwrap();
    let got = StorageRoot::from_tx(tx.tx_ref(), address).root().unwrap();

    assert_eq!(storage_root(storage.into_iter()), got);
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
                insert_account(tx.tx_ref(), *address, *account, storage)
            }
            tx.commit().unwrap();
            let tx =  factory.provider_rw().unwrap();

            let expected = state_root(state);

            let threshold = 10;
            let mut got = None;
            let mut hashed_entries_walked = 0;

            let mut intermediate_state: Option<Box<IntermediateStateRootState>> = None;
            while got.is_none() {
                let calculator = StateRoot::from_tx(tx.tx_ref())
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
            assert_eq!(expected, got.unwrap());
            assert_eq!(hashed_entries_total, hashed_entries_walked)
        }
    );
}

fn test_state_root_with_state(state: State) {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    for (address, (account, storage)) in &state {
        insert_account(tx.tx_ref(), *address, *account, storage)
    }
    tx.commit().unwrap();
    let expected = state_root(state);

    let tx = factory.provider_rw().unwrap();
    let got = StateRoot::from_tx(tx.tx_ref()).root().unwrap();
    assert_eq!(expected, got);
}

fn encode_account(account: Account, storage_root: Option<B256>) -> Vec<u8> {
    let account = TrieAccount::from((account, storage_root.unwrap_or(EMPTY_ROOT_HASH)));
    let mut account_rlp = Vec::with_capacity(account.length());
    account.encode(&mut account_rlp);
    account_rlp
}

#[test]
fn storage_root_regression() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    // Some address whose hash starts with 0xB041
    let address3 = Address::from_str("16b07afd1c635f77172e842a000ead9a2a222459").unwrap();
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
        hashed_storage_cursor.upsert(key3, StorageEntry { key: hashed_slot, value }).unwrap();
    }
    tx.commit().unwrap();
    let tx = factory.provider_rw().unwrap();

    let account3_storage_root = StorageRoot::from_tx(tx.tx_ref(), address3).root().unwrap();
    let expected_root = storage_root_prehashed(storage);
    assert_eq!(expected_root, account3_storage_root);
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
    let key1 =
        B256::from_str("b000000000000000000000000000000000000000000000000000000000000000").unwrap();
    let account1 = Account { nonce: 0, balance: U256::from(3).mul(ether), bytecode_hash: None };
    hashed_account_cursor.upsert(key1, account1).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key1), &encode_account(account1, None));

    // Some address whose hash starts with 0xB040
    let address2 = Address::from_str("7db3e81b72d2695e19764583f6d219dbee0f35ca").unwrap();
    let key2 = keccak256(address2);
    assert_eq!(key2[0], 0xB0);
    assert_eq!(key2[1], 0x40);
    let account2 = Account { nonce: 0, balance: ether, ..Default::default() };
    hashed_account_cursor.upsert(key2, account2).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key2), &encode_account(account2, None));

    // Some address whose hash starts with 0xB041
    let address3 = Address::from_str("16b07afd1c635f77172e842a000ead9a2a222459").unwrap();
    let key3 = keccak256(address3);
    assert_eq!(key3[0], 0xB0);
    assert_eq!(key3[1], 0x41);
    let code_hash =
        B256::from_str("5be74cad16203c4905c068b012a2e9fb6d19d036c410f16fd177f337541440dd").unwrap();
    let account3 =
        Account { nonce: 0, balance: U256::from(2).mul(ether), bytecode_hash: Some(code_hash) };
    hashed_account_cursor.upsert(key3, account3).unwrap();
    for (hashed_slot, value) in storage {
        if hashed_storage_cursor
            .seek_by_key_subkey(key3, hashed_slot)
            .unwrap()
            .filter(|e| e.key == hashed_slot)
            .is_some()
        {
            hashed_storage_cursor.delete_current().unwrap();
        }
        hashed_storage_cursor.upsert(key3, StorageEntry { key: hashed_slot, value }).unwrap();
    }
    let account3_storage_root = StorageRoot::from_tx(tx.tx_ref(), address3).root().unwrap();
    hash_builder
        .add_leaf(Nibbles::unpack(key3), &encode_account(account3, Some(account3_storage_root)));

    let key4a =
        B256::from_str("B1A0000000000000000000000000000000000000000000000000000000000000").unwrap();
    let account4a = Account { nonce: 0, balance: U256::from(4).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key4a, account4a).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key4a), &encode_account(account4a, None));

    let key5 =
        B256::from_str("B310000000000000000000000000000000000000000000000000000000000000").unwrap();
    let account5 = Account { nonce: 0, balance: U256::from(8).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key5, account5).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key5), &encode_account(account5, None));

    let key6 =
        B256::from_str("B340000000000000000000000000000000000000000000000000000000000000").unwrap();
    let account6 = Account { nonce: 0, balance: U256::from(1).mul(ether), ..Default::default() };
    hashed_account_cursor.upsert(key6, account6).unwrap();
    hash_builder.add_leaf(Nibbles::unpack(key6), &encode_account(account6, None));

    // Populate account & storage trie DB tables
    let expected_root =
        B256::from_str("72861041bc90cd2f93777956f058a545412b56de79af5eb6b8075fe2eabbe015").unwrap();
    let computed_expected_root: B256 = triehash::trie_root::<KeccakHasher, _, _, _>([
        (key1, encode_account(account1, None)),
        (key2, encode_account(account2, None)),
        (key3, encode_account(account3, Some(account3_storage_root))),
        (key4a, encode_account(account4a, None)),
        (key5, encode_account(account5, None)),
        (key6, encode_account(account6, None)),
    ]);
    // Check computed trie root to ensure correctness
    assert_eq!(computed_expected_root, expected_root);

    // Check hash builder root
    assert_eq!(hash_builder.root(), computed_expected_root);

    // Check state root calculation from scratch
    let (root, trie_updates) = StateRoot::from_tx(tx.tx_ref()).root_with_updates().unwrap();
    assert_eq!(root, computed_expected_root);

    // Check account trie
    let account_updates = trie_updates.into_sorted();
    let account_updates = account_updates.account_nodes_ref();
    assert_eq!(account_updates.len(), 2);

    let (nibbles1a, node1a) = account_updates.first().unwrap();
    assert_eq!(nibbles1a[..], [0xB]);
    assert_eq!(node1a.state_mask, TrieMask::new(0b1011));
    assert_eq!(node1a.tree_mask, TrieMask::new(0b0001));
    assert_eq!(node1a.hash_mask, TrieMask::new(0b1001));
    assert_eq!(node1a.root_hash, None);
    assert_eq!(node1a.hashes.len(), 2);

    let (nibbles2a, node2a) = account_updates.last().unwrap();
    assert_eq!(nibbles2a[..], [0xB, 0x0]);
    assert_eq!(node2a.state_mask, TrieMask::new(0b10001));
    assert_eq!(node2a.tree_mask, TrieMask::new(0b00000));
    assert_eq!(node2a.hash_mask, TrieMask::new(0b10000));
    assert_eq!(node2a.root_hash, None);
    assert_eq!(node2a.hashes.len(), 1);

    // Add an account
    // Some address whose hash starts with 0xB1
    let address4b = Address::from_str("4f61f2d5ebd991b85aa1677db97307caf5215c91").unwrap();
    let key4b = keccak256(address4b);
    assert_eq!(key4b.0[0], key4a.0[0]);
    let account4b = Account { nonce: 0, balance: U256::from(5).mul(ether), bytecode_hash: None };
    hashed_account_cursor.upsert(key4b, account4b).unwrap();

    let mut prefix_set = PrefixSetMut::default();
    prefix_set.insert(Nibbles::unpack(key4b));

    let expected_state_root =
        B256::from_str("8e263cd4eefb0c3cbbb14e5541a66a755cad25bcfab1e10dd9d706263e811b28").unwrap();

    let (root, trie_updates) = StateRoot::from_tx(tx.tx_ref())
        .with_prefix_sets(TriePrefixSets {
            account_prefix_set: prefix_set.freeze(),
            ..Default::default()
        })
        .root_with_updates()
        .unwrap();
    assert_eq!(root, expected_state_root);

    let account_updates = trie_updates.into_sorted();
    let account_updates = account_updates.account_nodes_ref();
    assert_eq!(account_updates.len(), 2);

    let (nibbles1b, node1b) = account_updates.first().unwrap();
    assert_eq!(nibbles1b[..], [0xB]);
    assert_eq!(node1b.state_mask, TrieMask::new(0b1011));
    assert_eq!(node1b.tree_mask, TrieMask::new(0b0001));
    assert_eq!(node1b.hash_mask, TrieMask::new(0b1011));
    assert_eq!(node1b.root_hash, None);
    assert_eq!(node1b.hashes.len(), 3);
    assert_eq!(node1a.hashes[0], node1b.hashes[0]);
    assert_eq!(node1a.hashes[1], node1b.hashes[2]);

    let (nibbles2b, node2b) = account_updates.last().unwrap();
    assert_eq!(nibbles2b[..], [0xB, 0x0]);
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
            (key1, encode_account(account1, None)),
            // DELETED: (key2, encode_account(account2, None)),
            (key3, encode_account(account3, Some(account3_storage_root))),
            (key4a, encode_account(account4a, None)),
            (key4b, encode_account(account4b, None)),
            (key5, encode_account(account5, None)),
            (key6, encode_account(account6, None)),
        ]);

        let (root, trie_updates) = StateRoot::from_tx(tx.tx_ref())
            .with_prefix_sets(TriePrefixSets {
                account_prefix_set: account_prefix_set.freeze(),
                ..Default::default()
            })
            .root_with_updates()
            .unwrap();
        assert_eq!(root, computed_expected_root);
        assert_eq!(
            trie_updates.account_nodes_ref().len() + trie_updates.removed_nodes_ref().len(),
            1
        );

        assert_eq!(trie_updates.account_nodes_ref().len(), 1);

        let (nibbles1c, node1c) = trie_updates.account_nodes_ref().iter().next().unwrap();
        assert_eq!(nibbles1c[..], [0xB]);

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
            (key1, encode_account(account1, None)),
            // DELETED: (key2, encode_account(account2, None)),
            // DELETED: (key3, encode_account(account3, Some(account3_storage_root))),
            (key4a, encode_account(account4a, None)),
            (key4b, encode_account(account4b, None)),
            (key5, encode_account(account5, None)),
            (key6, encode_account(account6, None)),
        ]);

        let (root, trie_updates) = StateRoot::from_tx(tx.tx_ref())
            .with_prefix_sets(TriePrefixSets {
                account_prefix_set: account_prefix_set.freeze(),
                ..Default::default()
            })
            .root_with_updates()
            .unwrap();
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

        let (nibbles1d, node1d) = trie_updates.account_nodes_ref().iter().next().unwrap();
        assert_eq!(nibbles1d[..], [0xB]);

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

    let (got, updates) = StateRoot::from_tx(tx.tx_ref()).root_with_updates().unwrap();
    assert_eq!(expected, got);
    assert_trie_updates(updates.account_nodes_ref());
}

#[test]
fn account_trie_around_extension_node_with_dbtrie() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let expected = extension_node_trie(&tx);

    let (got, updates) = StateRoot::from_tx(tx.tx_ref()).root_with_updates().unwrap();
    assert_eq!(expected, got);
    tx.write_trie_updates(&updates).unwrap();

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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128, ..ProptestConfig::default()
    })]

    #[test]
    fn fuzz_state_root_incremental(account_changes: [BTreeMap<B256, U256>; 5]) {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap();
        let mut hashed_account_cursor = tx.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

        let mut state = BTreeMap::default();
        for accounts in account_changes {
            let should_generate_changeset = !state.is_empty();
            let mut changes = PrefixSetMut::default();
            for (hashed_address, balance) in accounts.clone() {
                hashed_account_cursor.upsert(hashed_address, Account { balance, ..Default::default() }).unwrap();
                if should_generate_changeset {
                    changes.insert(Nibbles::unpack(hashed_address));
                }
            }

            let (state_root, trie_updates) = StateRoot::from_tx(tx.tx_ref())
                .with_prefix_sets(TriePrefixSets { account_prefix_set: changes.freeze(), ..Default::default() })
                .root_with_updates()
                .unwrap();

            state.append(&mut accounts.clone());
            let expected_root = state_root_prehashed(
                state.iter().map(|(&key, &balance)| (key, (Account { balance, ..Default::default() }, std::iter::empty())))
            );
            assert_eq!(expected_root, state_root);
            tx.write_trie_updates(&trie_updates).unwrap();
        }
    }
}

#[test]
fn storage_trie_around_extension_node() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let hashed_address = B256::random();
    let (expected_root, expected_updates) = extension_node_storage_trie(&tx, hashed_address);

    let (got, _, updates) =
        StorageRoot::from_tx_hashed(tx.tx_ref(), hashed_address).root_with_updates().unwrap();
    assert_eq!(expected_root, got);
    assert_eq!(expected_updates, updates);
    assert_trie_updates(updates.storage_nodes_ref());
}

fn extension_node_storage_trie<Spec: Send + Sync>(
    tx: &DatabaseProviderRW<Arc<TempDatabase<DatabaseEnv>>, Spec>,
    hashed_address: B256,
) -> (B256, StorageTrieUpdates) {
    let value = U256::from(1);

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
        hashed_storage.upsert(hashed_address, StorageEntry { key: B256::new(key), value }).unwrap();
        hb.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode_fixed_size(&value));
    }

    let root = hb.root();
    let (_, updates) = hb.split();
    let trie_updates = StorageTrieUpdates::new(updates);
    (root, trie_updates)
}

fn extension_node_trie<Spec: Send + Sync>(
    tx: &DatabaseProviderRW<Arc<TempDatabase<DatabaseEnv>>, Spec>,
) -> B256 {
    let a = Account { nonce: 0, balance: U256::from(1u64), bytecode_hash: Some(B256::random()) };
    let val = encode_account(a, None);

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
        hashed_accounts.upsert(B256::new(key), a).unwrap();
        hb.add_leaf(Nibbles::unpack(key), &val);
    }

    hb.root()
}

fn assert_trie_updates(account_updates: &HashMap<Nibbles, BranchNodeCompact>) {
    assert_eq!(account_updates.len(), 2);

    let node = account_updates.get(&[0x3][..]).unwrap();
    let expected = BranchNodeCompact::new(0b0011, 0b0001, 0b0000, vec![], None);
    assert_eq!(node, &expected);

    let node = account_updates.get(&[0x3, 0x0, 0xA, 0xF][..]).unwrap();
    assert_eq!(node.state_mask, TrieMask::new(0b101100000));
    assert_eq!(node.tree_mask, TrieMask::new(0b000000000));
    assert_eq!(node.hash_mask, TrieMask::new(0b001000000));

    assert_eq!(node.root_hash, None);
    assert_eq!(node.hashes.len(), 1);
}
