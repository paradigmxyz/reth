#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::B256;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use reth_trie_sparse::SparseStateTrie;

const HOT_ACCOUNTS: usize = 512;
const SLOTS_PER_ACCOUNT: usize = 16;
const HOT_SLOTS: usize = HOT_ACCOUNTS * SLOTS_PER_ACCOUNT;

fn benchmark_key(prefix: u64, suffix: u64) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&prefix.to_be_bytes());
    bytes[24..].copy_from_slice(&suffix.to_be_bytes());
    B256::from(bytes)
}

fn sparse_hotset(include_accounts: bool) -> SparseStateTrie {
    let mut sparse = SparseStateTrie::default();
    sparse
        .set_hot_cache_capacities(HOT_SLOTS, include_accounts.then_some(HOT_ACCOUNTS).unwrap_or(0));

    for account_idx in 0..HOT_ACCOUNTS as u64 {
        let account = benchmark_key(0xA11CE, account_idx);

        if include_accounts {
            sparse.record_account_touch(account);
        }

        for slot_idx in 0..SLOTS_PER_ACCOUNT as u64 {
            sparse.record_slot_touch(account, benchmark_key(account_idx + 1, slot_idx));
        }
    }

    sparse
}

fn bench_prune_hotset(c: &mut Criterion) {
    let mut group = c.benchmark_group("prune_hotset");
    group.sample_size(30);

    group.bench_function("retained_slots_by_address_hotset", |b| {
        let mut sparse = sparse_hotset(false);
        b.iter(|| black_box(&mut sparse).prune(HOT_SLOTS, 0));
    });

    group.bench_function("sparse_state_trie_prune_retained_hotset", |b| {
        let mut sparse = sparse_hotset(true);
        b.iter(|| black_box(&mut sparse).prune(HOT_SLOTS, HOT_ACCOUNTS));
    });

    group.finish();
}

criterion_group!(prune_hotset, bench_prune_hotset);
criterion_main!(prune_hotset);
