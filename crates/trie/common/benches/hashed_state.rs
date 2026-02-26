#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use reth_primitives_traits::Account;
use reth_trie_common::{HashedPostState, HashedStorage};

/// Generate test data: (`hashed_address`, account, storage)
fn generate_test_data(size: usize) -> Vec<(B256, Option<Account>, Option<HashedStorage>)> {
    (0..size)
        .map(|i| {
            let hashed_address = B256::from(keccak256_u64(i as u64));

            let account = if i % 10 == 0 {
                // 10% destroyed accounts
                None
            } else {
                Some(Account {
                    nonce: i as u64,
                    balance: U256::from(i * 1000),
                    bytecode_hash: None,
                })
            };

            let storage = (i % 3 == 0).then(|| {
                let mut storage = HashedStorage::new(false);
                // Add 5 storage slots per account with storage
                for j in 0..5 {
                    storage
                        .storage
                        .insert(B256::from(keccak256_u64((i * 100 + j) as u64)), U256::from(j));
                }
                storage
            });

            (hashed_address, account, storage)
        })
        .collect()
}

/// Simple keccak256 mock for benchmarking (to avoid crypto overhead in bench)
fn keccak256_u64(n: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&n.to_be_bytes());
    // XOR with some pattern to spread bits
    for (i, item) in bytes.iter_mut().enumerate() {
        *item ^= ((n.wrapping_mul(0x9e3779b97f4a7c15) >> (i * 8)) & 0xff) as u8;
    }
    bytes
}

/// Comparison implementation: fold + reduce with `HashMap` (not used, kept for benchmarking)
fn from_par_iter_fold_reduce(
    data: Vec<(B256, Option<Account>, Option<HashedStorage>)>,
) -> HashedPostState {
    data.into_par_iter()
        .fold(HashedPostState::default, |mut acc, (hashed_address, info, hashed_storage)| {
            acc.accounts.insert(hashed_address, info);
            if let Some(storage) = hashed_storage {
                acc.storages.insert(hashed_address, storage);
            }
            acc
        })
        .reduce(HashedPostState::default, |mut a, b| {
            a.extend(b);
            a
        })
}

/// Current implementation: collect to Vec (using rayon's optimized parallel collect), then
/// sequentially collect to `HashedPostState`
fn from_par_iter_collect_twice(
    data: Vec<(B256, Option<Account>, Option<HashedStorage>)>,
) -> HashedPostState {
    let vec: Vec<_> = data.into_par_iter().collect();
    vec.into_iter().collect()
}

fn bench_from_parallel_iterator(c: &mut Criterion) {
    let mut group = c.benchmark_group("HashedPostState::from_par_iter");

    for size in [100, 1_000, 10_000, 50_000] {
        let data = generate_test_data(size);

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("fold_reduce", size), &data, |b, data| {
            b.iter(|| {
                let result = from_par_iter_fold_reduce(black_box(data.clone()));
                black_box(result);
            });
        });

        group.bench_with_input(BenchmarkId::new("collect_twice", size), &data, |b, data| {
            b.iter(|| {
                let result = from_par_iter_collect_twice(black_box(data.clone()));
                black_box(result);
            });
        });
    }

    group.finish();
}

/// V1 global sort approach for chunking (baseline for comparison)
fn chunks_global_sort(state: HashedPostState, chunk_size: usize) -> Vec<HashedPostState> {
    use itertools::Itertools;

    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum Order {
        StorageWipe,
        StorageUpdate(B256),
        Account,
    }

    enum Item {
        Account(Option<Account>),
        StorageWipe,
        StorageUpdate { slot: B256, value: U256 },
    }

    impl Item {
        const fn order(&self) -> Order {
            match self {
                Self::StorageWipe => Order::StorageWipe,
                Self::StorageUpdate { slot, .. } => Order::StorageUpdate(*slot),
                Self::Account(_) => Order::Account,
            }
        }
    }

    let flattened: Vec<_> = state
        .storages
        .into_iter()
        .flat_map(|(address, storage)| {
            storage.wiped.then_some((address, Item::StorageWipe)).into_iter().chain(
                storage
                    .storage
                    .into_iter()
                    .map(move |(slot, value)| (address, Item::StorageUpdate { slot, value })),
            )
        })
        .chain(
            state.accounts.into_iter().map(|(address, account)| (address, Item::Account(account))),
        )
        .sorted_unstable_by_key(|(address, item)| (*address, item.order()))
        .collect();

    let mut chunks = Vec::new();
    let mut iter = flattened.into_iter();
    loop {
        let mut chunk = HashedPostState::default();
        let mut count = 0;
        while count < chunk_size {
            let Some((address, item)) = iter.next() else {
                break;
            };
            match item {
                Item::Account(account) => {
                    chunk.accounts.insert(address, account);
                }
                Item::StorageWipe => {
                    chunk.storages.entry(address).or_default().wiped = true;
                }
                Item::StorageUpdate { slot, value } => {
                    chunk.storages.entry(address).or_default().storage.insert(slot, value);
                }
            }
            count += 1;
        }
        if chunk.accounts.is_empty() && chunk.storages.is_empty() {
            break;
        }
        chunks.push(chunk);
    }
    chunks
}

fn generate_chunking_test_data(num_accounts: usize, slots_per_account: usize) -> HashedPostState {
    let mut state = HashedPostState::default();
    for i in 0..num_accounts {
        let hashed_address = B256::from(keccak256_u64(i as u64));
        state.accounts.insert(
            hashed_address,
            if i % 10 == 0 {
                None
            } else {
                Some(Account {
                    nonce: i as u64,
                    balance: U256::from(i * 1000),
                    bytecode_hash: None,
                })
            },
        );
        if slots_per_account > 0 {
            let mut storage = HashedStorage::new(i % 20 == 0);
            for j in 0..slots_per_account {
                storage
                    .storage
                    .insert(B256::from(keccak256_u64((i * 1000 + j) as u64)), U256::from(j));
            }
            state.storages.insert(hashed_address, storage);
        }
    }
    state
}

fn bench_chunking(c: &mut Criterion) {
    let mut group = c.benchmark_group("ChunkedHashedPostState");

    for (num_accounts, slots_per_account) in [(100, 5), (500, 10), (1000, 5), (200, 50)] {
        let chunk_size = 50;
        let label = format!("accounts_{num_accounts}/slots_{slots_per_account}/chunk_{chunk_size}");

        group.throughput(Throughput::Elements(
            (num_accounts + num_accounts * slots_per_account) as u64,
        ));

        group.bench_with_input(
            BenchmarkId::new("v1_global_sort", &label),
            &(num_accounts, slots_per_account, chunk_size),
            |b, &(na, spa, cs)| {
                b.iter_with_setup(
                    || generate_chunking_test_data(na, spa),
                    |state| {
                        let chunks = chunks_global_sort(black_box(state), cs);
                        black_box(chunks);
                    },
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("v2_merge_sort", &label),
            &(num_accounts, slots_per_account, chunk_size),
            |b, &(na, spa, cs)| {
                b.iter_with_setup(
                    || generate_chunking_test_data(na, spa),
                    |state| {
                        let chunks: Vec<_> = black_box(state).chunks(cs).collect();
                        black_box(chunks);
                    },
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_from_parallel_iterator, bench_chunking);
criterion_main!(benches);
