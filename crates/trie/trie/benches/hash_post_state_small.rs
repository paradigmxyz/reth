#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{map::AddressMap, Address, U256};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_trie::{HashedPostState, KeccakKeyHasher};
use revm_database::{states::BundleBuilder, BundleAccount};

/// Benchmark with ~10 storage slots per account (contract interactions).
pub fn hash_post_state_small_10slots(c: &mut Criterion) {
    let mut group = c.benchmark_group("from_bundle_state (10 slots/account)");
    group.sample_size(100);

    for size in [1, 5, 10, 20, 50, 75, 100, 200] {
        let state = generate_test_data(size, 10);

        group.bench_function(BenchmarkId::new("accounts", size), |b| {
            b.iter(|| HashedPostState::from_bundle_state::<KeccakKeyHasher>(&state))
        });
    }
}

/// Benchmark with ~2 storage slots per account (simple transfers).
pub fn hash_post_state_small_2slots(c: &mut Criterion) {
    let mut group = c.benchmark_group("from_bundle_state (2 slots/account)");
    group.sample_size(100);

    for size in [1, 5, 10, 20, 50, 75, 100, 200] {
        let state = generate_test_data(size, 2);

        group.bench_function(BenchmarkId::new("accounts", size), |b| {
            b.iter(|| HashedPostState::from_bundle_state::<KeccakKeyHasher>(&state))
        });
    }
}

fn generate_test_data(num_accounts: usize, storage_size: usize) -> AddressMap<BundleAccount> {
    let mut runner = TestRunner::deterministic();

    use proptest::collection::hash_map;
    let state = hash_map(
        any::<Address>(),
        hash_map(
            any::<U256>(), // slot
            (
                any::<U256>(), // old value
                any::<U256>(), // new value
            ),
            storage_size,
        ),
        num_accounts,
    )
    .new_tree(&mut runner)
    .unwrap()
    .current();

    let mut bundle_builder = BundleBuilder::default();

    for (address, storage) in state {
        bundle_builder = bundle_builder.state_storage(address, storage.into_iter().collect());
    }

    let bundle_state = bundle_builder.build();

    bundle_state.state.into_iter().collect()
}

criterion_group!(small_state, hash_post_state_small_10slots, hash_post_state_small_2slots);
criterion_main!(small_state);
