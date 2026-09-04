//! Sparse trie updates must survive persistence and remain usable by the database trie walker.

mod persistence_support;

use alloy_primitives::{keccak256, map::B256Map, B256, U256};
use persistence_support::{open_database, snapshot};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_trie::{
    test_utils::{state_root_prehashed, storage_root_prehashed},
    updates::StorageTrieUpdates,
    HashBuilder, Nibbles, StateRoot, StorageRoot,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseStorageRoot, DatabaseStorageTrieCursor,
    DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedKeyAdapter, TrieTableAdapter,
};
use reth_trie_sparse::{ArenaParallelSparseTrie, LeafUpdate, SparseTrie, TrieNodeEpoch};
use std::collections::BTreeMap;

fn storage(round: u64) -> BTreeMap<B256, U256> {
    (0u64..528)
        .filter(|i| match round {
            2 => i % 3 != 0,
            3 => *i >= 512,
            4 => false,
            _ => true,
        })
        .map(|i| {
            // The final keys share 63 nibbles, exercising embedded nodes and branch collapse.
            let key =
                if i < 512 { keccak256(i.to_be_bytes()) } else { B256::with_last_byte(i as u8) };
            let value = if i >= 512 { U256::ONE } else { U256::from(i + 1) << (round % 4 * 64) };
            (key, value)
        })
        .collect()
}

fn sparse_root_persistence<A: TrieTableAdapter>() {
    let dir = tempfile::tempdir().unwrap();
    let addresses = [0x10, 0x11, 0x12, 0x13].map(B256::repeat_byte);
    let mut tries = addresses.map(|_| {
        let mut trie = ArenaParallelSparseTrie::default();
        trie.set_updates(true);
        trie
    });
    let mut accounts = ArenaParallelSparseTrie::default();
    accounts.set_updates(true);
    let mut previous = BTreeMap::<B256, U256>::new();

    for round in 0..6 {
        let state = storage(round);
        let expected_storage_root = storage_root_prehashed(state.clone());
        let account = Account { nonce: round + 1, balance: U256::ONE, bytecode_hash: None };
        let expected_state_root = state_root_prehashed(
            addresses.iter().map(|address| (*address, (account, state.clone()))),
        );
        let mut expected_nodes = BTreeMap::new();
        let db = open_database(dir.path());
        let tx = db.tx_mut().unwrap();
        let mut account_updates = B256Map::default();
        for (address, trie) in addresses.iter().zip(&mut tries) {
            let mut changes: B256Map<_> = previous
                .keys()
                .filter(|key| !state.contains_key(*key))
                .map(|key| (*key, LeafUpdate::Changed(Vec::new())))
                .collect();
            changes.extend(
                state
                    .iter()
                    .map(|(key, value)| (*key, LeafUpdate::Changed(alloy_rlp::encode(value)))),
            );
            trie.update_leaves(&mut changes, |_, _| panic!("fully revealed trie")).unwrap();
            assert!(changes.is_empty());
            assert_eq!(trie.root(TrieNodeEpoch::new(round + 1)), expected_storage_root);

            let updates = trie.take_updates();
            let updates = StorageTrieUpdates {
                storage_nodes: updates.updated_nodes,
                removed_nodes: updates.removed_nodes,
            }
            .into_sorted();
            DatabaseStorageTrieCursor::<_, A>::new(
                tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                *address,
            )
            .write_storage_trie_updates_sorted(&updates)
            .unwrap();

            let mut flat = tx.cursor_dup_write::<tables::HashedStorages>().unwrap();
            if flat.seek_exact(*address).unwrap().is_some() {
                flat.delete_current_duplicates().unwrap();
            }
            let mut reference = HashBuilder::default().with_updates(true);
            for (key, value) in &state {
                flat.upsert(*address, &StorageEntry { key: *key, value: *value }).unwrap();
                reference.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode(value));
            }
            assert_eq!(reference.root(), expected_storage_root);
            // Reth persists intermediate branches; the empty root path is omitted.
            expected_nodes.extend(
                reference.split().1.into_iter().filter_map(|(path, node)| {
                    (!path.is_empty()).then_some(((*address, path), node))
                }),
            );
            tx.put::<tables::HashedAccounts>(*address, account).unwrap();
            account_updates.insert(
                *address,
                LeafUpdate::Changed(alloy_rlp::encode(
                    account.into_trie_account(expected_storage_root),
                )),
            );
        }

        accounts.update_leaves(&mut account_updates, |_, _| panic!("fully revealed trie")).unwrap();
        assert_eq!(accounts.root(TrieNodeEpoch::new(round + 1)), expected_state_root);
        let updates = accounts.take_updates();
        let mut cursor = tx.cursor_write::<A::AccountTrieTable>().unwrap();
        for path in updates.removed_nodes {
            if cursor.seek_exact(A::AccountKey::from(path)).unwrap().is_some() {
                cursor.delete_current().unwrap();
            }
        }
        for (path, node) in updates.updated_nodes {
            cursor.upsert(A::AccountKey::from(path), &node).unwrap();
        }
        drop(cursor);
        assert_eq!(snapshot::<A>(&tx), expected_nodes);
        tx.commit().unwrap();
        drop(db);

        let db = open_database(dir.path());
        let tx = db.tx().unwrap();
        assert_eq!(snapshot::<A>(&tx), expected_nodes);
        for address in addresses {
            let root = StorageRoot::<
                DatabaseTrieCursorFactory<_, A>,
                DatabaseHashedCursorFactory<_>,
            >::from_tx_hashed(&tx, address)
            .root()
            .unwrap();
            assert_eq!(root, expected_storage_root);
        }
        let root =
            StateRoot::<DatabaseTrieCursorFactory<_, A>, DatabaseHashedCursorFactory<_>>::from_tx(
                &tx,
            )
            .root()
            .unwrap();
        assert_eq!(root, expected_state_root);
        previous = state;
    }
}

#[test]
fn legacy_sparse_state_survives_reopen() {
    sparse_root_persistence::<LegacyKeyAdapter>();
}

#[test]
fn packed_sparse_state_survives_reopen() {
    sparse_root_persistence::<PackedKeyAdapter>();
}
