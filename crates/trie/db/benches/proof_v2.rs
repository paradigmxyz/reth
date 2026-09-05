#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{keccak256, map::B256Map, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use reth_db::test_utils::create_test_rw_db;
use reth_db_api::{
    tables,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_primitives_traits::{Account, StorageEntry};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    proof_v2::{ProofCalculator, StorageProofCalculator, SyncAccountValueEncoder},
    trie_cursor::TrieCursorFactory,
    StateRoot, StorageRoot,
};
use reth_trie_common::{
    prefix_set::PrefixSetMut, HashedPostState, HashedStorage, Nibbles, ProofV2Target,
    ProofV2TargetParent, StorageTrieEntry,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory, LegacyKeyAdapter};
use std::collections::BTreeMap;

fn key(index: usize) -> B256 {
    keccak256((index as u64).to_be_bytes())
}

fn proof_targets(keys: &[B256], mode: &str) -> Vec<ProofV2Target> {
    keys.iter()
        .enumerate()
        .map(|(index, &key)| {
            let parent = match mode {
                "Partial" => ProofV2TargetParent::new(2),
                "Mixed" => ProofV2TargetParent::new([0, 2, 8][index % 3]),
                _ => ProofV2TargetParent::NONE,
            };
            ProofV2Target::new(key).with_parent(parent)
        })
        .collect()
}

fn bench_database_proofs(c: &mut Criterion) {
    let mut group = c.benchmark_group("DatabaseProof");
    let address = B256::repeat_byte(0x42);
    for size in [128, 1024, 10240] {
        let mut keys = (0..size).map(key).collect::<Vec<_>>();
        keys.sort_unstable();
        for cache in ["cached", "leaves", "dirty"] {
            let db = create_test_rw_db();
            let tx = db.tx_mut().unwrap();
            for (index, &slot) in keys.iter().enumerate() {
                tx.put::<tables::HashedStorages>(
                    address,
                    StorageEntry { key: slot, value: U256::from(index + 1) },
                )
                .unwrap();
            }
            if cache != "leaves" {
                let (_, _, updates) = StorageRoot::new_hashed(
                    DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx),
                    DatabaseHashedCursorFactory::new(&tx),
                    address,
                    Default::default(),
                    #[cfg(feature = "metrics")]
                    reth_trie::metrics::TrieRootMetrics::new(reth_trie::TrieType::Storage),
                )
                .root_with_updates()
                .unwrap();
                for (path, node) in updates.storage_nodes {
                    tx.put::<tables::StoragesTrie>(
                        address,
                        StorageTrieEntry { nibbles: path.into(), node },
                    )
                    .unwrap();
                }
            }
            tx.commit().unwrap();
            let tx = db.tx().unwrap();

            let mut changes = B256Map::default();
            if cache == "dirty" {
                for (index, &slot) in keys.iter().enumerate().step_by(16) {
                    changes.insert(
                        slot,
                        if index % 32 == 0 { U256::ZERO } else { U256::from(size + index) },
                    );
                    changes.insert(key(size + index), U256::from(index + 1));
                }
            }
            let mut prefixes = PrefixSetMut::default();
            for &slot in changes.keys() {
                prefixes.insert(Nibbles::unpack(slot));
            }
            let prefixes = prefixes.freeze();
            let mut current = keys
                .iter()
                .enumerate()
                .map(|(index, &key)| (key, U256::from(index + 1)))
                .collect::<BTreeMap<_, _>>();
            for (&key, &value) in &changes {
                if value.is_zero() {
                    current.remove(&key);
                } else {
                    current.insert(key, value);
                }
            }
            let expected_root = reth_trie::test_utils::storage_root_prehashed(current);
            let overlay = HashedPostState {
                storages: B256Map::from_iter([(address, HashedStorage { storage: changes })]),
                ..Default::default()
            }
            .into_sorted();
            let trie_factory = DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx);
            let hashed_factory =
                HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(&tx), &overlay);

            for count in [1, 16, 128, 2048] {
                let keys =
                    (0..count)
                        .map(|index| {
                            if index % 5 == 0 {
                                key(size + index)
                            } else {
                                keys[(index * 13) % size]
                            }
                        })
                        .collect::<Vec<_>>();
                for mode in ["Full", "Partial", "Mixed"] {
                    let mut calculator = StorageProofCalculator::new_storage(
                        trie_factory.storage_trie_cursor(address).unwrap(),
                        hashed_factory.hashed_storage_cursor(address).unwrap(),
                    )
                    .with_prefix_set(prefixes.clone());
                    group.bench_function(
                        BenchmarkId::new(
                            format!("{cache}/{mode}"),
                            format!("dataset_{size}/targets_{count}"),
                        ),
                        |b| {
                            if mode == "Full" {
                                let mut targets = keys
                                    .iter()
                                    .copied()
                                    .map(ProofV2Target::new)
                                    .collect::<Vec<_>>();
                                let proof =
                                    calculator.storage_proof(address, &mut targets).unwrap();
                                assert_eq!(
                                    calculator.compute_root_hash(&proof).unwrap(),
                                    Some(expected_root)
                                );
                            }
                            b.iter_batched(
                                || proof_targets(&keys, mode),
                                |mut targets| {
                                    calculator.storage_proof(address, &mut targets).unwrap()
                                },
                                BatchSize::SmallInput,
                            )
                        },
                    );
                }
            }
            let mut calculator = StorageProofCalculator::new_storage(
                trie_factory.storage_trie_cursor(address).unwrap(),
                hashed_factory.hashed_storage_cursor(address).unwrap(),
            )
            .with_prefix_set(prefixes.clone());
            group.bench_function(
                BenchmarkId::new(format!("{cache}/Root"), format!("dataset_{size}")),
                |b| {
                    let root = calculator.storage_root_node(address).unwrap();
                    assert_eq!(calculator.compute_root_hash(&[root]).unwrap(), Some(expected_root));
                    b.iter(|| calculator.storage_root_node(address).unwrap());
                },
            );
        }
    }
}

fn bench_database_account_proofs(c: &mut Criterion) {
    let mut group = c.benchmark_group("DatabaseAccountProof");
    let size = 1024;
    let mut accounts = (0..size)
        .map(|index| {
            (
                key(index),
                Account {
                    nonce: index as u64,
                    balance: U256::from(index + 1),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    accounts.sort_unstable_by_key(|(address, _)| *address);
    for slots in [0, 16] {
        let storage =
            (0..slots).map(|index| (key(index), U256::from(index + 1))).collect::<Vec<_>>();
        let expected_root = reth_trie::test_utils::state_root_prehashed(
            accounts
                .iter()
                .map(|&(address, account)| (address, (account, storage.iter().copied()))),
        );
        for cache in ["cached", "leaves"] {
            let db = create_test_rw_db();
            let tx = db.tx_mut().unwrap();
            for &(address, account) in &accounts {
                tx.put::<tables::HashedAccounts>(address, account).unwrap();
                for &(key, value) in &storage {
                    tx.put::<tables::HashedStorages>(address, StorageEntry { key, value }).unwrap();
                }
            }
            if cache == "cached" {
                let (root, updates) = StateRoot::new(
                    DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx),
                    DatabaseHashedCursorFactory::new(&tx),
                )
                .root_with_updates()
                .unwrap();
                assert_eq!(root, expected_root);
                for (path, node) in updates.account_nodes {
                    tx.put::<tables::AccountsTrie>(path.into(), node).unwrap();
                }
                for (address, updates) in updates.storage_tries {
                    for (path, node) in updates.storage_nodes {
                        tx.put::<tables::StoragesTrie>(
                            address,
                            StorageTrieEntry { nibbles: path.into(), node },
                        )
                        .unwrap();
                    }
                }
            }
            tx.commit().unwrap();
            let tx = db.tx().unwrap();
            let overlay = HashedPostState::default().into_sorted();
            let trie_factory = DatabaseTrieCursorFactory::<_, LegacyKeyAdapter>::new(&tx);
            let hashed_factory =
                HashedPostStateCursorFactory::new(DatabaseHashedCursorFactory::new(&tx), &overlay);
            let mut encoder =
                SyncAccountValueEncoder::new(trie_factory.clone(), hashed_factory.clone());
            for count in [1, 16, 128] {
                let keys = (0..count)
                    .map(|index| {
                        if index % 5 == 0 {
                            key(size + index)
                        } else {
                            accounts[(index * 13) % size].0
                        }
                    })
                    .collect::<Vec<_>>();
                for mode in ["Full", "Partial", "Mixed"] {
                    let mut calculator = ProofCalculator::new(
                        trie_factory.account_trie_cursor().unwrap(),
                        hashed_factory.hashed_account_cursor().unwrap(),
                    );
                    group.bench_function(
                        BenchmarkId::new(
                            format!("{cache}/{mode}"),
                            format!("accounts_{size}/slots_{slots}/targets_{count}"),
                        ),
                        |b| {
                            if mode == "Full" {
                                let proof = calculator
                                    .proof(&mut encoder, &mut proof_targets(&keys, mode))
                                    .unwrap();
                                assert_eq!(
                                    calculator.compute_root_hash(&proof).unwrap(),
                                    Some(expected_root)
                                );
                            }
                            b.iter_batched(
                                || proof_targets(&keys, mode),
                                |mut targets| calculator.proof(&mut encoder, &mut targets).unwrap(),
                                BatchSize::SmallInput,
                            )
                        },
                    );
                }
            }
        }
    }
}

criterion_group!(proofs, bench_database_proofs, bench_database_account_proofs);
criterion_main!(proofs);
