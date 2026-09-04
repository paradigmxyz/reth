//! Differential operation-stream tests for persisted storage branch updates.

mod persistence_support;

use alloy_primitives::B256;
use persistence_support::{baseline_write, node, open_database, path, snapshot, wipe};
use proptest::prelude::*;
use reth_db_api::{
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_trie::{updates::StorageTrieUpdatesSorted, BranchNodeCompact, Nibbles};
use reth_trie_db::{
    DatabaseStorageTrieCursor, LegacyKeyAdapter, PackedKeyAdapter, TrieTableAdapter,
};
use std::collections::BTreeMap;

type Update = (u8, Nibbles, Option<BranchNodeCompact>);
type Batch = (Option<u8>, Vec<Update>);

fn check_batches<A: TrieTableAdapter>(batches: &[Batch]) {
    let reference_dir = tempfile::tempdir().unwrap();
    let optimized_dir = tempfile::tempdir().unwrap();
    let mut reference = open_database(reference_dir.path());
    let mut optimized = open_database(optimized_dir.path());
    let mut expected = BTreeMap::new();

    for (wiped, updates) in batches {
        let reference_tx = reference.tx_mut().unwrap();
        let optimized_tx = optimized.tx_mut().unwrap();
        if let Some(address) = wiped {
            let address = B256::with_last_byte(*address);
            wipe::<A>(&reference_tx, address);
            wipe::<A>(&optimized_tx, address);
            expected.retain(|(key, _), _| *key != address);
        }

        let mut sorted = BTreeMap::<_, BTreeMap<_, _>>::new();
        for (address, path, update) in updates {
            let address = B256::with_last_byte(*address);
            sorted.entry(address).or_default().insert(*path, update.clone());
            if !path.is_empty() {
                if let Some(node) = update {
                    expected.insert((address, *path), node.clone());
                } else {
                    expected.remove(&(address, *path));
                }
            }
        }

        for (address, updates) in sorted {
            let updates = StorageTrieUpdatesSorted { storage_nodes: updates.into_iter().collect() };
            let count = baseline_write::<_, A>(
                &mut reference_tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                address,
                &updates,
            )
            .unwrap();
            let mut cursor = DatabaseStorageTrieCursor::<_, A>::new(
                optimized_tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                address,
            );
            assert_eq!(cursor.write_storage_trie_updates_sorted(&updates).unwrap(), count);
        }
        assert_eq!(snapshot::<A>(&reference_tx), expected);
        assert_eq!(snapshot::<A>(&optimized_tx), expected);
        reference_tx.commit().unwrap();
        optimized_tx.commit().unwrap();

        // Close the environments, then compare decoded branches after a durable commit and reopen.
        drop(reference);
        drop(optimized);
        reference = open_database(reference_dir.path());
        optimized = open_database(optimized_dir.path());
        assert_eq!(snapshot::<A>(&reference.tx().unwrap()), expected);
        assert_eq!(snapshot::<A>(&optimized.tx().unwrap()), expected);
    }
}

#[test]
fn dense_updates_sparse_gaps_and_appended_nodes() {
    let initial = (0..512).map(|i| (1, path(i * 2), Some(node(u64::from(i), 8)))).collect();
    let replacement =
        (0..512).map(|i| (1, path(i * 2), Some(node(u64::from(i + 512), 16)))).collect();
    let sparse = (0..32).map(|i| (1, path(i * 32), Some(node(u64::from(i), 2)))).collect();
    let gaps = (0..512).map(|i| (1, path(i * 2 + 1), Some(node(u64::from(i), 4)))).collect();
    let appended = (1024..1536).map(|i| (1, path(i), Some(node(u64::from(i), 8)))).collect();
    let removals = (0..768).map(|i| (1, path(i * 2), None)).collect();
    let batches = [initial, replacement, sparse, gaps, appended, removals]
        .into_iter()
        .map(|updates| (None, updates))
        .collect::<Vec<_>>();
    check_batches::<LegacyKeyAdapter>(&batches);
    check_batches::<PackedKeyAdapter>(&batches);
}

#[test]
fn repeated_paths_follow_prior_updates() {
    fn check<A: TrieTableAdapter>() {
        let batches = [
            vec![(path(1), Some(node(1, 8))), (path(2), Some(node(2, 8)))],
            vec![
                (path(1), Some(node(3, 16))),
                (path(1), Some(node(4, 2))),
                (path(2), None),
                (path(2), Some(node(5, 8))),
                (path(3), None),
                (path(3), Some(node(6, 8))),
                (path(3), None),
                (path(4), Some(node(7, 8))),
            ],
        ];
        let reference_dir = tempfile::tempdir().unwrap();
        let optimized_dir = tempfile::tempdir().unwrap();
        let reference = open_database(reference_dir.path());
        let optimized = open_database(optimized_dir.path());
        let address = B256::with_last_byte(1);
        for storage_nodes in batches {
            let updates = StorageTrieUpdatesSorted { storage_nodes };
            let reference_tx = reference.tx_mut().unwrap();
            let optimized_tx = optimized.tx_mut().unwrap();
            let count = baseline_write::<_, A>(
                &mut reference_tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                address,
                &updates,
            )
            .unwrap();
            assert_eq!(
                DatabaseStorageTrieCursor::<_, A>::new(
                    optimized_tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
                    address,
                )
                .write_storage_trie_updates_sorted(&updates)
                .unwrap(),
                count
            );
            assert_eq!(snapshot::<A>(&optimized_tx), snapshot::<A>(&reference_tx));
            reference_tx.commit().unwrap();
            optimized_tx.commit().unwrap();
        }
        assert_eq!(
            snapshot::<A>(&optimized.tx().unwrap()),
            snapshot::<A>(&reference.tx().unwrap())
        );
    }
    check::<LegacyKeyAdapter>();
    check::<PackedKeyAdapter>();
}

#[test]
fn aborted_storage_updates_leave_committed_state_unchanged() {
    fn check<A: TrieTableAdapter>(explicit_abort: bool) {
        let dir = tempfile::tempdir().unwrap();
        let db = open_database(dir.path());
        let address = B256::with_last_byte(1);
        let initial = StorageTrieUpdatesSorted {
            storage_nodes: (0..16)
                .map(|i| (path(i), Some(node(u64::from(i), (i % 3 * 8) as u8))))
                .collect(),
        };
        let tx = db.tx_mut().unwrap();
        baseline_write::<_, A>(
            &mut tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
            address,
            &initial,
        )
        .unwrap();
        let expected = snapshot::<A>(&tx);
        tx.commit().unwrap();

        let updates = StorageTrieUpdatesSorted {
            storage_nodes: vec![
                (path(0), None),
                (path(1), Some(node(100, 16))),
                (path(2), Some(node(101, 0))),
                (path(3), Some(node(102, 0))),
                (path(99), Some(node(103, 8))),
            ],
        };
        let tx = db.tx_mut().unwrap();
        DatabaseStorageTrieCursor::<_, A>::new(
            tx.cursor_dup_write::<A::StorageTrieTable>().unwrap(),
            address,
        )
        .write_storage_trie_updates_sorted(&updates)
        .unwrap();
        assert_ne!(snapshot::<A>(&tx), expected);
        if explicit_abort {
            tx.abort();
        } else {
            drop(tx);
        }
        assert_eq!(snapshot::<A>(&db.tx().unwrap()), expected);
        drop(db);
        let db = open_database(dir.path());
        assert_eq!(snapshot::<A>(&db.tx().unwrap()), expected);
    }

    for explicit_abort in [false, true] {
        check::<LegacyKeyAdapter>(explicit_abort);
        check::<PackedKeyAdapter>(explicit_abort);
    }
}

#[test]
fn replacement_resize_delete_wipe_and_reopen() {
    let keys = [
        Nibbles::default(),
        Nibbles::from_nibbles([0]),
        Nibbles::from_nibbles([0, 0]),
        Nibbles::from_nibbles([0, 0, 1]),
        Nibbles::from_nibbles([0, 1]),
        Nibbles::from_nibbles([15; 63]),
        Nibbles::from_nibbles([15; 64]),
    ];
    let batch = |seed, hashes| keys.iter().map(|key| (1, *key, Some(node(seed, hashes)))).collect();
    let batches = vec![
        (None, batch(1, 1)),
        (None, batch(2, 16)),
        (None, batch(3, 0)),
        (None, batch(3, 0)),
        (None, keys.iter().map(|key| (1, *key, None)).collect()),
        (None, vec![(1, path(1), None), (2, path(1), Some(node(4, 2)))]),
        (None, batch(5, 4)),
        (Some(1), vec![(1, path(100), Some(node(6, 16)))]),
        (Some(1), vec![]),
        (Some(1), vec![]),
    ];
    check_batches::<LegacyKeyAdapter>(&batches);
    check_batches::<PackedKeyAdapter>(&batches);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn differential_storage_operation_stream(
        input in prop::collection::vec(
            (
                prop::option::of(0u8..4),
                prop::collection::vec((0u8..4, 0u32..64, any::<u64>(), 0u8..17, any::<bool>()), 0..64),
            ),
            1..9,
        )
    ) {
        let batches = input.into_iter().map(|(wipe, updates)| {
            (wipe, updates.into_iter().map(|(address, key, seed, hashes, remove)| {
                (address, path(key), (!remove).then(|| node(seed, hashes)))
            }).collect())
        }).collect::<Vec<_>>();
        check_batches::<LegacyKeyAdapter>(&batches);
        check_batches::<PackedKeyAdapter>(&batches);
    }
}
