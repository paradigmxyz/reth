#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use reth_primitives_traits::Account;
use reth_trie_common::{HashedPostState, HashedPostStateSorted, HashedStorage, HashedStorageSorted};
use std::collections::HashMap;

fn keccak256_mock(n: u64) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&n.to_be_bytes());
    for (i, item) in bytes.iter_mut().enumerate() {
        *item ^= ((n.wrapping_mul(0x9e3779b97f4a7c15) >> (i * 8)) & 0xff) as u8;
    }
    B256::from(bytes)
}

fn generate_hashed_post_state_sorted(
    base_offset: u64,
    num_accounts: usize,
    storage_slots_per_account: usize,
) -> HashedPostStateSorted {
    let mut state = HashedPostState::default();

    for i in 0..num_accounts {
        let hashed_address = keccak256_mock(base_offset + i as u64);
        let account = Some(Account {
            nonce: i as u64,
            balance: U256::from(i * 1000),
            bytecode_hash: None,
        });
        state.accounts.insert(hashed_address, account);

        if storage_slots_per_account > 0 {
            let mut storage = HashedStorage::new(false);
            for j in 0..storage_slots_per_account {
                let slot = keccak256_mock(base_offset * 1000 + (i * 100 + j) as u64);
                storage.storage.insert(slot, U256::from(j));
            }
            state.storages.insert(hashed_address, storage);
        }
    }

    state.into_sorted()
}

fn generate_states(
    num_sources: usize,
    items_per_source: usize,
) -> Vec<HashedPostStateSorted> {
    (0..num_sources)
        .map(|i| {
            generate_hashed_post_state_sorted(
                (i * 1_000_000) as u64,
                items_per_source,
                5,
            )
        })
        .collect()
}

fn extend_ref_chain(states: &[HashedPostStateSorted]) -> HashedPostStateSorted {
    if states.is_empty() {
        return HashedPostStateSorted::default();
    }
    let mut result = states[0].clone();
    for state in &states[1..] {
        result.extend_ref(state);
    }
    result
}

fn bench_merge_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("HashedPostStateSorted_merge");

    // Test various source counts and items per source
    // We want to find where extend_ref beats kway_merge
    let configs = [
        // (num_sources, items_per_source)
        (5, 100),
        (5, 500),
        (5, 1000),
        (5, 2000),
        (5, 5000),
        (5, 10000),
        (10, 100),
        (10, 500),
        (10, 1000),
        (10, 2000),
        (10, 5000),
        (20, 100),
        (20, 500),
        (20, 1000),
        (20, 2000),
    ];

    for (num_sources, items_per_source) in configs {
        let states = generate_states(num_sources, items_per_source);
        let total_items = num_sources * items_per_source;
        let label = format!("{}src_{}items", num_sources, items_per_source);

        group.throughput(Throughput::Elements(total_items as u64));

        group.bench_with_input(
            BenchmarkId::new("kway_merge", &label),
            &states,
            |b, states| {
                b.iter(|| {
                    let result = HashedPostStateSorted::merge_batch(black_box(states.iter()));
                    black_box(result)
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("extend_ref_new", &label),
            &states,
            |b, states| {
                b.iter(|| {
                    let result = extend_ref_chain(black_box(states));
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_merge_strategies);
criterion_main!(benches);
