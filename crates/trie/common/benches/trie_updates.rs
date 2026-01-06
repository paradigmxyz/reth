#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::B256;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_trie_common::{
    updates::{StorageTrieUpdates, TrieUpdates},
    BranchNodeCompact, Nibbles,
};

/// Generate test data for TrieUpdates
fn generate_trie_updates(num_accounts: usize, nodes_per_account: usize) -> TrieUpdates {
    let mut updates = TrieUpdates::default();

    // Add account nodes
    for i in 0..num_accounts {
        let path = Nibbles::from_nibbles_unchecked([(i % 16) as u8, ((i / 16) % 16) as u8]);
        updates.account_nodes.insert(path, BranchNodeCompact::default());
    }

    // Add some removed nodes (20% of accounts)
    for i in 0..num_accounts / 5 {
        let path =
            Nibbles::from_nibbles_unchecked([((i + 8) % 16) as u8, ((i / 16 + 8) % 16) as u8]);
        updates.removed_nodes.insert(path);
    }

    // Add storage tries
    for i in 0..num_accounts {
        let hashed_address = B256::from(keccak256_u64(i as u64));
        let mut storage = StorageTrieUpdates::default();

        // Add storage nodes
        for j in 0..nodes_per_account {
            let path = Nibbles::from_nibbles_unchecked([(j % 16) as u8, ((j / 16) % 16) as u8]);
            storage.storage_nodes.insert(path, BranchNodeCompact::default());
        }

        // Add some removed storage nodes (10% of storage)
        for j in 0..nodes_per_account / 10 {
            let path =
                Nibbles::from_nibbles_unchecked([((j + 4) % 16) as u8, ((j / 16 + 4) % 16) as u8]);
            storage.removed_nodes.insert(path);
        }

        updates.storage_tries.insert(hashed_address, storage);
    }

    updates
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

fn bench_trie_updates_sorting(c: &mut Criterion) {
    let mut group = c.benchmark_group("TrieUpdates::sorting");

    // Test with different sizes to see how the optimization scales
    for (num_accounts, nodes_per_account) in
        [(10, 5), (50, 10), (100, 20), (500, 50), (1000, 100), (2000, 150)]
    {
        let updates = generate_trie_updates(num_accounts, nodes_per_account);
        let total_nodes = num_accounts + num_accounts * nodes_per_account;

        group.throughput(Throughput::Elements(total_nodes as u64));

        // Benchmark: clone().into_sorted() - the old way
        group.bench_with_input(
            BenchmarkId::new(
                "clone_then_into_sorted",
                format!("{}acc_{}nodes", num_accounts, nodes_per_account),
            ),
            &updates,
            |b, updates: &TrieUpdates| {
                b.iter(|| {
                    let result = black_box(updates.clone()).into_sorted();
                    black_box(result);
                });
            },
        );

        // Benchmark: clone_into_sorted() - the new optimized way
        group.bench_with_input(
            BenchmarkId::new(
                "clone_into_sorted",
                format!("{}acc_{}nodes", num_accounts, nodes_per_account),
            ),
            &updates,
            |b, updates: &TrieUpdates| {
                b.iter(|| {
                    let result = black_box(updates).clone_into_sorted();
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

fn bench_storage_trie_updates_sorting(c: &mut Criterion) {
    let mut group = c.benchmark_group("StorageTrieUpdates::sorting");

    for num_nodes in [10, 50, 100, 500, 1000] {
        let mut storage = StorageTrieUpdates::default();

        // Add storage nodes
        for i in 0..num_nodes {
            let path = Nibbles::from_nibbles_unchecked([(i % 16) as u8, ((i / 16) % 16) as u8]);
            storage.storage_nodes.insert(path, BranchNodeCompact::default());
        }

        // Add some removed nodes (20% of nodes)
        for i in 0..num_nodes / 5 {
            let path =
                Nibbles::from_nibbles_unchecked([((i + 8) % 16) as u8, ((i / 16 + 8) % 16) as u8]);
            storage.removed_nodes.insert(path);
        }

        group.throughput(Throughput::Elements(num_nodes as u64));

        // Benchmark: clone().into_sorted() - the old way
        group.bench_with_input(
            BenchmarkId::new("clone_then_into_sorted", num_nodes),
            &storage,
            |b, storage: &StorageTrieUpdates| {
                b.iter(|| {
                    let result = black_box(storage.clone()).into_sorted();
                    black_box(result);
                });
            },
        );

        // Benchmark: clone_into_sorted() - the new optimized way
        group.bench_with_input(
            BenchmarkId::new("clone_into_sorted", num_nodes),
            &storage,
            |b, storage: &StorageTrieUpdates| {
                b.iter(|| {
                    let result = black_box(storage).clone_into_sorted();
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_trie_updates_sorting, bench_storage_trie_updates_sorting);
criterion_main!(benches);
