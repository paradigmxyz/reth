#![allow(missing_docs)]

use alloy_primitives::{keccak256, B256};
use reth_db::test_utils::create_test_rw_db;
use reth_db_api::{
    cursor::DbCursorRO,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_trie_common::{updates::StorageTrieUpdatesSorted, BranchNodeCompact, Nibbles};
use reth_trie_db::{
    DatabaseStorageTrieCursor, LegacyKeyAdapter, PackedKeyAdapter, StorageTrieEntryLike,
    TrieTableAdapter,
};
use std::collections::BTreeMap;

fn check_storage_updates<A: TrieTableAdapter>() {
    let db = create_test_rw_db();
    let mut expected = BTreeMap::new();
    for round in 0..7u16 {
        let tx = db.tx_mut().unwrap();
        for address in [B256::with_last_byte(1), B256::with_last_byte(2)] {
            let mut nodes = Vec::new();
            for index in 0..288u16 {
                let path = Nibbles::unpack(index.to_be_bytes());
                let remove =
                    round == 6 || (round == 3 && index % 3 == 0) || (round < 4 && index >= 256);
                let count = match round {
                    1 => 0,
                    2 => 16,
                    _ => usize::from(index % 16 + 1),
                };
                let mask = if count == 16 { u16::MAX } else { (1u16 << count) - 1 };
                let node = (!remove).then(|| {
                    let hashes = (0..count)
                        .map(|child| keccak256([round as u8, index as u8, child as u8]))
                        .collect();
                    BranchNodeCompact::new(
                        u16::MAX,
                        mask,
                        mask,
                        hashes,
                        (round % 2 == 0).then(|| keccak256([round as u8, index as u8])),
                    )
                });
                if let Some(node) = &node {
                    expected.insert((address, path), node.clone());
                } else {
                    expected.remove(&(address, path));
                }
                nodes.push((path, node));
            }
            let updates = StorageTrieUpdatesSorted { storage_nodes: nodes };
            let mut cursor = DatabaseStorageTrieCursor::<_, A>::new(
                tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                address,
            );
            assert_eq!(cursor.write_storage_trie_updates_sorted(&updates).unwrap(), 288);
            assert_eq!(cursor.write_storage_trie_updates_sorted(&updates).unwrap(), 288);
        }
        tx.commit().unwrap();
        let tx = db.tx().unwrap();
        let actual = tx
            .cursor_read::<A::StorageTrieTable>()
            .unwrap()
            .walk(None)
            .unwrap()
            .map(|entry| {
                let (address, value) = entry.unwrap();
                let (subkey, node) = value.into_parts();
                ((address, A::subkey_to_nibbles(&subkey)), node)
            })
            .collect::<Vec<_>>();
        let expected_rows =
            expected.iter().map(|(key, node)| (*key, node.clone())).collect::<Vec<_>>();
        assert_eq!(actual, expected_rows, "round {round}");
    }
}

#[test]
fn legacy_storage_updates_preserve_other_nodes() {
    check_storage_updates::<LegacyKeyAdapter>();
}

#[test]
fn packed_storage_updates_preserve_other_nodes() {
    check_storage_updates::<PackedKeyAdapter>();
}
