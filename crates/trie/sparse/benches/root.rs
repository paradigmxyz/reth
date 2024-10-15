#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{map::HashMap, B256, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use itertools::Itertools;
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie::{
    hashed_cursor::{noop::NoopHashedStorageCursor, HashedPostStateStorageCursor},
    node_iter::{TrieElement, TrieNodeIter},
    trie_cursor::{noop::NoopStorageTrieCursor, InMemoryStorageTrieCursor},
    updates::StorageTrieUpdates,
    walker::TrieWalker,
    HashedStorage,
};
use reth_trie_common::{HashBuilder, Nibbles};
use reth_trie_sparse::SparseTrie;

pub fn calculate_root_from_leaves(c: &mut Criterion) {
    let mut group = c.benchmark_group("calculate root from leaves");
    group.sample_size(20);

    for size in [1_000, 5_000, 10_000, 100_000] {
        let state = generate_test_data(size);

        // hash builder
        group.bench_function(BenchmarkId::new("hash builder", size), |b| {
            b.iter_with_setup(HashBuilder::default, |mut hb| {
                for (key, value) in state.iter().sorted_by_key(|(key, _)| *key) {
                    hb.add_leaf(Nibbles::unpack(key), &alloy_rlp::encode_fixed_size(value));
                }
                hb.root();
            })
        });

        // sparse trie
        group.bench_function(BenchmarkId::new("sparse trie", size), |b| {
            b.iter_with_setup(SparseTrie::revealed_empty, |mut sparse| {
                for (key, value) in &state {
                    sparse
                        .update_leaf(
                            Nibbles::unpack(key),
                            alloy_rlp::encode_fixed_size(value).to_vec(),
                        )
                        .unwrap();
                }
                sparse.root().unwrap();
            })
        });
    }
}

pub fn calculate_root_from_leaves_repeated(c: &mut Criterion) {
    let mut group = c.benchmark_group("calculate root from leaves repeated");
    group.sample_size(20);

    for init_size in [1_000, 10_000, 100_000] {
        let init_state = generate_test_data(init_size);

        for update_size in [100, 1_000, 5_000, 10_000] {
            for num_updates in [1, 3, 5, 10] {
                let updates =
                    (0..num_updates).map(|_| generate_test_data(update_size)).collect::<Vec<_>>();

                // hash builder
                let benchmark_id = BenchmarkId::new(
                    "hash builder",
                    format!("init size {init_size} | update size {update_size} | num updates {num_updates}"),
                );
                group.bench_function(benchmark_id, |b| {
                    b.iter_with_setup(
                        || {
                            let init_storage = HashedStorage::from_iter(false, init_state.clone());
                            let storage_updates = updates
                                .clone()
                                .into_iter()
                                .map(|update| HashedStorage::from_iter(false, update))
                                .collect::<Vec<_>>();

                            let mut hb = HashBuilder::default().with_updates(true);
                            for (key, value) in init_state.iter().sorted_by_key(|(key, _)| *key) {
                                hb.add_leaf(
                                    Nibbles::unpack(key),
                                    &alloy_rlp::encode_fixed_size(value),
                                );
                            }
                            hb.root();

                            let (_, updates) = hb.split();
                            let trie_updates = StorageTrieUpdates::new(updates);
                            (init_storage, storage_updates, trie_updates)
                        },
                        |(init_storage, storage_updates, mut trie_updates)| {
                            let mut storage = init_storage;
                            for update in storage_updates {
                                storage.extend(&update);

                                let prefix_set = update.construct_prefix_set().freeze();
                                let storage_sorted = storage.clone().into_sorted();
                                let trie_updates_sorted = trie_updates.clone().into_sorted();

                                let walker = TrieWalker::new(
                                    InMemoryStorageTrieCursor::new(
                                        B256::ZERO,
                                        NoopStorageTrieCursor::default(),
                                        Some(&trie_updates_sorted),
                                    ),
                                    prefix_set,
                                );
                                let mut node_iter = TrieNodeIter::new(
                                    walker,
                                    HashedPostStateStorageCursor::new(
                                        NoopHashedStorageCursor::default(),
                                        Some(&storage_sorted),
                                    ),
                                );

                                let mut hb = HashBuilder::default().with_updates(true);
                                while let Some(node) = node_iter.try_next().unwrap() {
                                    match node {
                                        TrieElement::Branch(node) => {
                                            hb.add_branch(
                                                node.key,
                                                node.value,
                                                node.children_are_in_trie,
                                            );
                                        }
                                        TrieElement::Leaf(hashed_slot, value) => {
                                            hb.add_leaf(
                                                Nibbles::unpack(hashed_slot),
                                                alloy_rlp::encode_fixed_size(&value).as_ref(),
                                            );
                                        }
                                    }
                                }
                                hb.root();

                                trie_updates.finalize(node_iter.walker, hb);
                            }
                        },
                    )
                });

                // sparse trie
                let benchmark_id = BenchmarkId::new(
                    "sparse trie",
                    format!("init size {init_size} | update size {update_size} | num updates {num_updates}"),
                );
                group.bench_function(benchmark_id, |b| {
                    b.iter_with_setup(
                        || {
                            let mut sparse = SparseTrie::revealed_empty();
                            for (key, value) in &init_state {
                                sparse
                                    .update_leaf(
                                        Nibbles::unpack(key),
                                        alloy_rlp::encode_fixed_size(value).to_vec(),
                                    )
                                    .unwrap();
                            }
                            sparse.root().unwrap();
                            sparse
                        },
                        |mut sparse| {
                            for update in &updates {
                                for (key, value) in update {
                                    sparse
                                        .update_leaf(
                                            Nibbles::unpack(key),
                                            alloy_rlp::encode_fixed_size(value).to_vec(),
                                        )
                                        .unwrap();
                                }
                                sparse.root().unwrap();
                            }
                        },
                    )
                });
            }
        }
    }
}

fn generate_test_data(size: usize) -> HashMap<B256, U256> {
    let mut runner = TestRunner::new(ProptestConfig::default());
    proptest::collection::hash_map(any::<B256>(), any::<U256>(), size)
        .new_tree(&mut runner)
        .unwrap()
        .current()
}

criterion_group!(root, calculate_root_from_leaves, calculate_root_from_leaves_repeated);
criterion_main!(root);
