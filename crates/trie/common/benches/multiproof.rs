#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{
    keccak256,
    map::{hash_map, B256Map},
    B256,
};
use alloy_trie::{nodes::TrieNode, proof::DecodedProofNodes};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_trie_common::{BranchNodeMasksMap, DecodedMultiProof, DecodedStorageMultiProof, Nibbles};

/// Baseline implementation without `reserve()` calls - for comparison
fn extend_baseline(base: &mut DecodedMultiProof, other: DecodedMultiProof) {
    base.account_subtree.extend_from(other.account_subtree);
    // NO reserve call
    base.branch_node_masks.extend(other.branch_node_masks);

    // NO reserve call
    for (hashed_address, storage) in other.storages {
        match base.storages.entry(hashed_address) {
            hash_map::Entry::Occupied(mut entry) => {
                let entry = entry.get_mut();
                entry.subtree.extend_from(storage.subtree);
                // NO reserve call
                entry.branch_node_masks.extend(storage.branch_node_masks);
            }
            hash_map::Entry::Vacant(entry) => {
                entry.insert(storage);
            }
        }
    }
}

/// Baseline batch implementation without upfront reserve
fn extend_batch_baseline(
    base: &mut DecodedMultiProof,
    others: impl IntoIterator<Item = DecodedMultiProof>,
) {
    for other in others {
        extend_baseline(base, other);
    }
}

/// Generate a random B256 from a seed
fn random_b256(seed: u64) -> B256 {
    keccak256(seed.to_le_bytes())
}

/// Generate random nibbles from a seed
fn random_nibbles(seed: u64) -> Nibbles {
    Nibbles::unpack(random_b256(seed))
}

/// Create a mock [`DecodedStorageMultiProof`] with the given number of entries
fn create_storage_multiproof(num_entries: usize, seed: u64) -> DecodedStorageMultiProof {
    let mut subtree = DecodedProofNodes::default();
    let mut branch_node_masks = BranchNodeMasksMap::default();

    for i in 0..num_entries {
        let nibbles = random_nibbles(seed + i as u64);
        subtree.insert(nibbles, TrieNode::EmptyRoot);
        branch_node_masks.insert(nibbles, Default::default());
    }

    DecodedStorageMultiProof { root: random_b256(seed), subtree, branch_node_masks }
}

/// Create a mock [`DecodedMultiProof`] with the given number of accounts and storage entries per
/// account
fn create_multiproof(
    num_accounts: usize,
    storage_entries_per_account: usize,
    seed: u64,
) -> DecodedMultiProof {
    let mut account_subtree = DecodedProofNodes::default();
    let mut branch_node_masks = BranchNodeMasksMap::default();
    let mut storages = B256Map::default();

    for i in 0..num_accounts {
        let account_seed = seed + (i * 1000) as u64;
        let hashed_address = random_b256(account_seed);
        let nibbles = Nibbles::unpack(hashed_address);

        account_subtree.insert(nibbles, TrieNode::EmptyRoot);
        branch_node_masks.insert(nibbles, Default::default());

        if storage_entries_per_account > 0 {
            storages.insert(
                hashed_address,
                create_storage_multiproof(storage_entries_per_account, account_seed + 500),
            );
        }
    }

    DecodedMultiProof { account_subtree, branch_node_masks, storages }
}

/// Benchmark extend with varying sizes - baseline vs optimized
fn bench_extend(c: &mut Criterion) {
    let mut group = c.benchmark_group("DecodedMultiProof::extend");
    group.sample_size(50);

    for num_accounts in [50, 100] {
        for storage_per_account in [10, 30] {
            let id = format!("accounts={num_accounts}/storage={storage_per_account}");
            group.throughput(Throughput::Elements((num_accounts * storage_per_account) as u64));

            // Baseline (no reserve)
            group.bench_with_input(
                BenchmarkId::new("baseline", &id),
                &(num_accounts, storage_per_account),
                |b, &(accounts, storage)| {
                    b.iter_with_setup(
                        || {
                            let base = create_multiproof(accounts, storage, 1);
                            let other = create_multiproof(accounts, storage, 100000);
                            (base, other)
                        },
                        |(mut base, other)| {
                            extend_baseline(&mut base, black_box(other));
                            base
                        },
                    );
                },
            );

            // Optimized (with reserve)
            group.bench_with_input(
                BenchmarkId::new("optimized", &id),
                &(num_accounts, storage_per_account),
                |b, &(accounts, storage)| {
                    b.iter_with_setup(
                        || {
                            let base = create_multiproof(accounts, storage, 1);
                            let other = create_multiproof(accounts, storage, 100000);
                            (base, other)
                        },
                        |(mut base, other)| {
                            base.extend(black_box(other));
                            base
                        },
                    );
                },
            );
        }
    }

    group.finish();
}

/// Benchmark `extend_swap` with varying sizes - baseline extend vs swap-extend
fn bench_extend_swap(c: &mut Criterion) {
    let mut group = c.benchmark_group("DecodedMultiProof::extend_swap");
    group.sample_size(50);

    // Test case where other is larger - swap should help
    for (self_accounts, other_accounts) in [(20, 80), (80, 20)] {
        let storage = 20;
        let id = format!("self={self_accounts}/other={other_accounts}");
        group.throughput(Throughput::Elements(((self_accounts + other_accounts) * storage) as u64));

        // Baseline extend
        group.bench_with_input(
            BenchmarkId::new("baseline", &id),
            &(self_accounts, other_accounts, storage),
            |b, &(self_acc, other_acc, stor)| {
                b.iter_with_setup(
                    || {
                        let base = create_multiproof(self_acc, stor, 1);
                        let other = create_multiproof(other_acc, stor, 100000);
                        (base, other)
                    },
                    |(mut base, other)| {
                        base.extend(black_box(other));
                        base
                    },
                );
            },
        );

        // Swap-extend
        group.bench_with_input(
            BenchmarkId::new("swap_extend", &id),
            &(self_accounts, other_accounts, storage),
            |b, &(self_acc, other_acc, stor)| {
                b.iter_with_setup(
                    || {
                        let base = create_multiproof(self_acc, stor, 1);
                        let other = create_multiproof(other_acc, stor, 100000);
                        (base, other)
                    },
                    |(mut base, other)| {
                        base.extend_swap(black_box(other));
                        base
                    },
                );
            },
        );
    }

    group.finish();
}

/// Benchmark `extend_batch` vs sequential extend
fn bench_extend_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("DecodedMultiProof::extend_batch");
    group.sample_size(50);

    for num_proofs in [10, 20] {
        let accounts_per_proof = 20;
        let storage_per_account = 10;

        let id = format!("proofs={num_proofs}");
        group.throughput(Throughput::Elements(
            (num_proofs * accounts_per_proof * storage_per_account) as u64,
        ));

        // Baseline: sequential extend without reserve
        group.bench_with_input(
            BenchmarkId::new("baseline_sequential", &id),
            &num_proofs,
            |b, &n| {
                b.iter_with_setup(
                    || {
                        let proofs: Vec<_> = (0..n)
                            .map(|i| {
                                create_multiproof(
                                    accounts_per_proof,
                                    storage_per_account,
                                    (i * 10000) as u64,
                                )
                            })
                            .collect();
                        proofs
                    },
                    |proofs| {
                        let mut iter = proofs.into_iter();
                        let mut acc = iter.next().unwrap();
                        extend_batch_baseline(&mut acc, black_box(iter));
                        acc
                    },
                );
            },
        );

        // Optimized: extend_batch with upfront reserve
        group.bench_with_input(BenchmarkId::new("optimized_batch", &id), &num_proofs, |b, &n| {
            b.iter_with_setup(
                || {
                    let proofs: Vec<_> = (0..n)
                        .map(|i| {
                            create_multiproof(
                                accounts_per_proof,
                                storage_per_account,
                                (i * 10000) as u64,
                            )
                        })
                        .collect();
                    proofs
                },
                |proofs| {
                    let mut iter = proofs.into_iter();
                    let mut acc = iter.next().unwrap();
                    acc.extend_batch(black_box(iter));
                    acc
                },
            );
        });
    }

    group.finish();
}

criterion_group!(multiproof_benches, bench_extend, bench_extend_swap, bench_extend_batch);
criterion_main!(multiproof_benches);
