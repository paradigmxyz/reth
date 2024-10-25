#![allow(missing_docs)]

use alloy_primitives::{keccak256, Address, B256, U256};
use reth_db::{tables, test_utils::create_test_rw_db, Database, HashedAccounts};
use reth_db_api::{cursor::DbCursorRW, transaction::DbTxMut};
use reth_primitives::Account;
use reth_provider::{test_utils::create_test_provider_factory, ProviderError};
use reth_trie::{
    prefix_set::{PrefixSetMut, TriePrefixSets},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory},
    updates::TrieUpdates,
    walker::TrieWalker,
    StorageTrieEntry,
};
use reth_trie_common::{BranchNodeCompact, Nibbles};
use reth_trie_db::{
    DatabaseAccountTrieCursor, DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
use std::sync::Arc;

#[test]
fn walk_nodes_with_common_prefix() {
    let inputs = vec![
        (vec![0x5u8], BranchNodeCompact::new(0b1_0000_0101, 0b1_0000_0100, 0, vec![], None)),
        (vec![0x5u8, 0x2, 0xC], BranchNodeCompact::new(0b1000_0111, 0, 0, vec![], None)),
        (vec![0x5u8, 0x8], BranchNodeCompact::new(0b0110, 0b0100, 0, vec![], None)),
    ];
    let expected = vec![
        vec![0x5, 0x0],
        // The [0x5, 0x2] prefix is shared by the first 2 nodes, however:
        // 1. 0x2 for the first node points to the child node path
        // 2. 0x2 for the second node is a key.
        // So to proceed to add 1 and 3, we need to push the sibling first (0xC).
        vec![0x5, 0x2],
        vec![0x5, 0x2, 0xC, 0x0],
        vec![0x5, 0x2, 0xC, 0x1],
        vec![0x5, 0x2, 0xC, 0x2],
        vec![0x5, 0x2, 0xC, 0x7],
        vec![0x5, 0x8],
        vec![0x5, 0x8, 0x1],
        vec![0x5, 0x8, 0x2],
    ];

    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();

    let mut account_cursor = tx.tx_ref().cursor_write::<tables::AccountsTrie>().unwrap();
    for (k, v) in &inputs {
        account_cursor.upsert(k.clone().into(), v.clone()).unwrap();
    }
    let account_trie = DatabaseAccountTrieCursor::new(account_cursor);
    test_cursor(account_trie, &expected);

    let hashed_address = B256::random();
    let mut storage_cursor = tx.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();
    for (k, v) in &inputs {
        storage_cursor
            .upsert(hashed_address, StorageTrieEntry { nibbles: k.clone().into(), node: v.clone() })
            .unwrap();
    }
    let storage_trie = DatabaseStorageTrieCursor::new(storage_cursor, hashed_address);
    test_cursor(storage_trie, &expected);
}

fn test_cursor<T>(mut trie: T, expected: &[Vec<u8>])
where
    T: TrieCursor,
{
    let mut walker = TrieWalker::new(&mut trie, Default::default());
    assert!(walker.key().unwrap().is_empty());

    // We're traversing the path in lexicographical order.
    for expected in expected {
        let got = walker.advance().unwrap();
        assert_eq!(got.unwrap(), Nibbles::from_nibbles_unchecked(expected.clone()));
    }

    // There should be 8 paths traversed in total from 3 branches.
    let got = walker.advance().unwrap();
    assert!(got.is_none());
}

#[test]
fn cursor_rootnode_with_changesets() {
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    let mut cursor = tx.tx_ref().cursor_dup_write::<tables::StoragesTrie>().unwrap();

    let nodes = vec![
        (
            vec![],
            BranchNodeCompact::new(
                // 2 and 4 are set
                0b10100,
                0b00100,
                0,
                vec![],
                Some(B256::random()),
            ),
        ),
        (
            vec![0x2],
            BranchNodeCompact::new(
                // 1 is set
                0b00010,
                0,
                0b00010,
                vec![B256::random()],
                None,
            ),
        ),
    ];

    let hashed_address = B256::random();
    for (k, v) in nodes {
        cursor.upsert(hashed_address, StorageTrieEntry { nibbles: k.into(), node: v }).unwrap();
    }

    let mut trie = DatabaseStorageTrieCursor::new(cursor, hashed_address);

    // No changes
    let mut cursor = TrieWalker::new(&mut trie, Default::default());
    assert_eq!(cursor.key().cloned(), Some(Nibbles::new())); // root
    assert!(cursor.can_skip_current_node); // due to root_hash
    cursor.advance().unwrap(); // skips to the end of trie
    assert_eq!(cursor.key().cloned(), None);

    // We insert something that's not part of the existing trie/prefix.
    let mut changed = PrefixSetMut::default();
    changed.insert(Nibbles::from_nibbles([0xF, 0x1]));
    let mut cursor = TrieWalker::new(&mut trie, changed.freeze());

    // Root node
    assert_eq!(cursor.key().cloned(), Some(Nibbles::new()));
    // Should not be able to skip state due to the changed values
    assert!(!cursor.can_skip_current_node);
    cursor.advance().unwrap();
    assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles([0x2])));
    cursor.advance().unwrap();
    assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles([0x2, 0x1])));
    cursor.advance().unwrap();
    assert_eq!(cursor.key().cloned(), Some(Nibbles::from_nibbles([0x4])));

    cursor.advance().unwrap();
    assert_eq!(cursor.key().cloned(), None); // the end of trie
}

#[test]
fn test_trie_walker_with_real_db_populated_account() {
    let db = create_test_rw_db();

    // Create test data
    let address1 = Address::random();
    let account1 = Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None };

    let tx = db.db().tx_mut().unwrap();

    // Add account to the database
    tx.put::<HashedAccounts>(B256::from(keccak256(address1)), account1)
        .expect("Failed to insert account");

    // Commit the transaction
    tx.inner.commit().expect("Failed to commit transaction");

    // Create a read transaction for testing
    let tx = db.tx().expect("Failed to create transaction");

    // Test account trie walker
    let trie_updates = TrieUpdates::default();
    let trie_nodes_sorted = Arc::new(trie_updates.into_sorted());

    let trie_cursor_factory =
        InMemoryTrieCursorFactory::new(DatabaseTrieCursorFactory::new(&tx), &trie_nodes_sorted);

    let prefix_sets = TriePrefixSets::default();
    let mut walker = TrieWalker::new(
        trie_cursor_factory.account_trie_cursor().map_err(ProviderError::Database).unwrap(),
        prefix_sets.account_prefix_set,
    );

    assert!(walker.key().is_some());
    let result = walker.advance().expect("Failed to advance walker");
    assert!(result.is_some());
}

/*
    #[test]
    fn test_trie_walker_with_real_db_populated_storage() {
        let db = create_test_rw_db();

        // Create test data
        let address1 = Address::random();
        let account1 = Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None };

        let tx = db.db().tx_mut().unwrap();

        // Add storage entry
        let storage_key = B256::random();
        let storage_value = StorageEntry { key: storage_key, value: U256::from(42) };
        tx.put::<HashedStorages>(B256::from(keccak256(address1)), storage_value)
            .expect("Failed to insert storage");

        // Commit the transaction
        tx.inner.commit().expect("Failed to commit transaction");

        // Create a read transaction for testing
        let tx = db.tx().expect("Failed to create transaction");

        let mut changes = PrefixSet::default();

        // Test storage trie walker
        let storage_cursor =
            tx.cursor_read::<HashedStorages>().expect("Failed to create storage cursor");
        let mut walker = TrieWalker::new(storage_cursor, changes);

        assert!(walker.key().is_some());
        let result = walker.advance().expect("Failed to advance walker");
        assert!(result.is_some());
}
    */
