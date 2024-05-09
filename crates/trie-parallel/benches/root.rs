#![allow(missing_docs, unreachable_pub)]
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use rayon::ThreadPoolBuilder;
use reth_primitives::{Account, B256, U256};
use reth_provider::{
    bundle_state::HashedStateChanges, providers::ConsistentDbView,
    test_utils::create_test_provider_factory,
};
use reth_tasks::pool::BlockingTaskPool;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, HashedPostState, HashedStorage, StateRoot,
};
use reth_trie_parallel::{async_root::AsyncStateRoot, parallel_root::ParallelStateRoot};
use std::collections::HashMap;

pub fn calculate_state_root(c: &mut Criterion) {
    let mut group = c.benchmark_group("Calculate State Root");
    group.sample_size(20);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let blocking_pool = BlockingTaskPool::new(ThreadPoolBuilder::default().build().unwrap());

    for size in [1_000, 3_000, 5_000, 10_000] {
        let (db_state, updated_state) = generate_test_data(size);
        let provider_factory = create_test_provider_factory();
        {
            let provider_rw = provider_factory.provider_rw().unwrap();
            HashedStateChanges(db_state).write_to_db(provider_rw.tx_ref()).unwrap();
            let (_, updates) =
                StateRoot::from_tx(provider_rw.tx_ref()).root_with_updates().unwrap();
            updates.flush(provider_rw.tx_ref()).unwrap();
            provider_rw.commit().unwrap();
        }

        let view = ConsistentDbView::new(provider_factory.clone(), None);

        // state root
        group.bench_function(BenchmarkId::new("sync root", size), |b| {
            b.to_async(&runtime).iter_with_setup(
                || {
                    let sorted_state = updated_state.clone().into_sorted();
                    let prefix_sets = updated_state.construct_prefix_sets();
                    let provider = provider_factory.provider().unwrap();
                    (provider, sorted_state, prefix_sets)
                },
                |(provider, sorted_state, prefix_sets)| async move {
                    StateRoot::from_tx(provider.tx_ref())
                        .with_hashed_cursor_factory(HashedPostStateCursorFactory::new(
                            provider.tx_ref(),
                            &sorted_state,
                        ))
                        .with_prefix_sets(prefix_sets)
                        .root()
                },
            )
        });

        // parallel root
        group.bench_function(BenchmarkId::new("parallel root", size), |b| {
            b.to_async(&runtime).iter_with_setup(
                || ParallelStateRoot::new(view.clone(), updated_state.clone()),
                |calculator| async { calculator.incremental_root() },
            );
        });

        // async root
        group.bench_function(BenchmarkId::new("async root", size), |b| {
            b.to_async(&runtime).iter_with_setup(
                || AsyncStateRoot::new(view.clone(), blocking_pool.clone(), updated_state.clone()),
                |calculator| calculator.incremental_root(),
            );
        });
    }
}

fn generate_test_data(size: usize) -> (HashedPostState, HashedPostState) {
    let storage_size = 1_000;
    let mut runner = TestRunner::new(ProptestConfig::default());

    use proptest::{collection::hash_map, sample::subsequence};
    let db_state = hash_map(
        any::<B256>(),
        (
            any::<Account>().prop_filter("non empty account", |a| !a.is_empty()),
            hash_map(
                any::<B256>(),
                any::<U256>().prop_filter("non zero value", |v| !v.is_zero()),
                storage_size,
            ),
        ),
        size,
    )
    .new_tree(&mut runner)
    .unwrap()
    .current();

    let keys = db_state.keys().cloned().collect::<Vec<_>>();
    let keys_to_update = subsequence(keys, size / 2).new_tree(&mut runner).unwrap().current();

    let updated_storages = keys_to_update
        .into_iter()
        .map(|address| {
            let (_, storage) = db_state.get(&address).unwrap();
            let slots = storage.keys().cloned().collect::<Vec<_>>();
            let slots_to_update =
                subsequence(slots, storage_size / 2).new_tree(&mut runner).unwrap().current();
            (
                address,
                slots_to_update
                    .into_iter()
                    .map(|slot| (slot, any::<U256>().new_tree(&mut runner).unwrap().current()))
                    .collect::<HashMap<_, _>>(),
            )
        })
        .collect::<HashMap<_, _>>();

    (
        HashedPostState::default()
            .with_accounts(
                db_state.iter().map(|(address, (account, _))| (*address, Some(*account))),
            )
            .with_storages(db_state.into_iter().map(|(address, (_, storage))| {
                (address, HashedStorage::from_iter(false, storage))
            })),
        HashedPostState::default().with_storages(
            updated_storages
                .into_iter()
                .map(|(address, storage)| (address, HashedStorage::from_iter(false, storage))),
        ),
    )
}

criterion_group!(state_root, calculate_state_root);
criterion_main!(state_root);
