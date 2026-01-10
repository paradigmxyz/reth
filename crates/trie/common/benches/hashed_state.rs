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

criterion_group!(benches, bench_from_parallel_iterator);
criterion_main!(benches);
