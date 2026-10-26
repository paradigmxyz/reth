//! Regression coverage for database-backed, bounded-stream trie verification.

use alloy_primitives::{keccak256, B256, U256};
use reth_db::{test_utils::create_test_rw_db, Database};
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_trie::{verify::Output, StateRoot, StorageRoot};
use reth_trie_common::{StorageTrieEntry, StoredNibbles, StoredNibblesSubKey};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseStorageRoot, DatabaseTrieCursorFactory,
    DatabaseVerifier, LegacyKeyAdapter,
};
use std::{collections::BTreeMap, sync::mpsc, thread, time::Duration};

type DbStateRoot<'a, TX> = StateRoot<
    DatabaseTrieCursorFactory<&'a TX, LegacyKeyAdapter>,
    DatabaseHashedCursorFactory<&'a TX>,
>;
type DbStorageRoot<'a, TX> = StorageRoot<
    DatabaseTrieCursorFactory<&'a TX, LegacyKeyAdapter>,
    DatabaseHashedCursorFactory<&'a TX>,
>;

#[test]
fn pristine_tries_have_no_findings() {
    completes(|| {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        insert_accounts(&tx, 64);
        insert_storage(&tx, B256::ZERO, 256);
        let (_, updates) = DbStateRoot::from_tx(&tx).root_with_updates().unwrap();
        for (path, node) in updates.account_nodes_ref() {
            tx.put::<tables::AccountsTrie>(StoredNibbles(*path), node.clone()).unwrap();
        }
        for (address, storage) in updates.storage_tries_ref() {
            for (path, node) in storage.storage_nodes_ref() {
                tx.put::<tables::StoragesTrie>(
                    *address,
                    StorageTrieEntry { nibbles: StoredNibblesSubKey(*path), node: node.clone() },
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        let outputs = DatabaseVerifier::<_, LegacyKeyAdapter>::new(db.as_ref(), 2, 1)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!outputs.is_empty(), "the pristine account walk must report progress");
        assert!(outputs.iter().all(|output| matches!(output, Output::Progress(_))), "{outputs:?}");
    });
}

#[test]
fn large_missing_account_trie_streams_every_node() {
    completes(|| {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        insert_accounts(&tx, 32_768);
        let (_, updates) = DbStateRoot::from_tx(&tx).root_with_updates().unwrap();
        let expected = updates
            .account_nodes_ref()
            .iter()
            .map(|(path, node)| (*path, node.clone()))
            .collect::<BTreeMap<_, _>>();
        assert!(expected.len() > 2_048);
        tx.commit().unwrap();
        let mut found = BTreeMap::new();
        for output in DatabaseVerifier::<_, LegacyKeyAdapter>::new(db.as_ref(), 2, 1).unwrap() {
            match output.unwrap() {
                Output::AccountMissing(path, node) => {
                    assert!(found.insert(path, node).is_none());
                    // Deliberately yield between findings without depending on elapsed time.
                    thread::yield_now();
                }
                Output::Progress(_) => {}
                output => panic!("unexpected output: {output:?}"),
            }
        }
        assert_eq!(found, expected);
    });
}

#[test]
fn large_missing_storage_streams_before_account_root_and_for_orphans() {
    completes(|| {
        for account_backed in [true, false] {
            let db = create_test_rw_db();
            let tx = db.tx_mut().unwrap();
            let address = B256::ZERO;
            if account_backed {
                insert_accounts(&tx, 64);
            }
            insert_storage(&tx, address, 32_768);
            let (_, _, updates) =
                DbStorageRoot::from_tx_hashed(&tx, address).root_with_updates().unwrap();
            let expected = updates
                .storage_nodes_ref()
                .iter()
                .map(|(path, node)| (*path, node.clone()))
                .collect::<BTreeMap<_, _>>();
            assert!(expected.len() > 2_048);
            let expected_progress = if account_backed {
                let (_, account_updates) = DbStateRoot::from_tx(&tx).root_with_updates().unwrap();
                for (path, node) in account_updates.account_nodes_ref() {
                    tx.put::<tables::AccountsTrie>(StoredNibbles(*path), node.clone()).unwrap();
                }
                account_updates.account_nodes_ref().len()
            } else {
                0
            };
            tx.commit().unwrap();
            let mut progress = 0;
            let mut found = BTreeMap::new();
            for output in DatabaseVerifier::<_, LegacyKeyAdapter>::new(db.as_ref(), 2, 1).unwrap() {
                match output.unwrap() {
                    Output::StorageMissing(found_address, path, node) => {
                        assert_eq!(found_address, address);
                        assert!(found.insert(path, node).is_none());
                        thread::yield_now();
                    }
                    Output::Progress(_) => progress += 1,
                    output => panic!("unexpected output: {output:?}"),
                }
            }
            assert_eq!(found, expected);
            assert_eq!(progress, expected_progress);
            assert_eq!(progress > 0, account_backed);
        }
    });
}

#[test]
fn stale_only_storage_trie_reports_every_extra_node() {
    completes(|| {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let address = B256::ZERO;
        insert_storage(&tx, address, 32_768);
        let (_, _, updates) =
            DbStorageRoot::from_tx_hashed(&tx, address).root_with_updates().unwrap();
        let expected = updates
            .storage_nodes_ref()
            .iter()
            .map(|(path, node)| (*path, node.clone()))
            .collect::<BTreeMap<_, _>>();
        assert!(expected.len() > 2_048);
        for (path, node) in &expected {
            tx.put::<tables::StoragesTrie>(
                address,
                StorageTrieEntry { nibbles: StoredNibblesSubKey(*path), node: node.clone() },
            )
            .unwrap();
        }
        tx.clear::<tables::HashedStorages>().unwrap();
        tx.commit().unwrap();
        let mut found = BTreeMap::new();
        for output in DatabaseVerifier::<_, LegacyKeyAdapter>::new(db.as_ref(), 2, 1).unwrap() {
            match output.unwrap() {
                Output::StorageExtra(found_address, path, node) => {
                    assert_eq!(found_address, address);
                    assert!(found.insert(path, node).is_none());
                }
                output => panic!("unexpected output: {output:?}"),
            }
        }
        assert_eq!(found, expected);
    });
}

#[test]
fn dropping_large_unfinished_verification_joins_producers() {
    completes(|| {
        for account_backed in [true, false] {
            let db = create_test_rw_db();
            let tx = db.tx_mut().unwrap();
            if account_backed {
                tx.put::<tables::HashedAccounts>(B256::ZERO, Account::default()).unwrap();
            }
            insert_storage(&tx, B256::ZERO, 32_768);
            tx.commit().unwrap();
            let mut verifier =
                DatabaseVerifier::<_, LegacyKeyAdapter>::new(db.as_ref(), 2, 1).unwrap();
            assert!(matches!(verifier.next().unwrap().unwrap(), Output::StorageMissing(..)));
            drop(verifier);
        }
    });
}

fn insert_accounts(tx: &impl DbTxMut, count: u64) {
    // Include address zero so the pristine fixture exercises account-backed storage.
    tx.put::<tables::HashedAccounts>(B256::ZERO, Account::default()).unwrap();
    for index in 1..count {
        tx.put::<tables::HashedAccounts>(keccak256(index.to_be_bytes()), Account::default())
            .unwrap();
    }
}

fn insert_storage(tx: &impl DbTxMut, address: B256, count: u64) {
    for index in 0..count {
        tx.put::<tables::HashedStorages>(
            address,
            StorageEntry { key: keccak256(index.to_be_bytes()), value: U256::from(index + 1) },
        )
        .unwrap();
    }
}

fn completes(work: impl FnOnce() + Send + 'static) {
    let (done, completion) = mpsc::channel();
    let worker = thread::spawn(move || {
        work();
        done.send(()).unwrap();
    });
    completion.recv_timeout(Duration::from_secs(120)).expect("verification failed to finish");
    worker.join().unwrap();
}
