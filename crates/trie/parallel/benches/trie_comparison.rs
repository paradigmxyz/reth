#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::{map::B256Map, B256, U256};
use alloy_rlp::Encodable;
use alloy_trie::root::ordered_trie_root_with_encoder;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_provider::{providers::ConsistentDbView, test_utils::create_test_provider_factory};
use reth_trie::{HashedPostState, HashedStorage, TrieInput};
use reth_trie_common::Nibbles;
use reth_trie_parallel::root::ParallelStateRoot;
use reth_trie_sparse::{provider::DefaultTrieNodeProvider, SerialSparseTrie, SparseTrie};

/// Compare different trie root calculation methods
pub fn compare_trie_implementations(c: &mut Criterion) {
    let mut group = c.benchmark_group("Trie Root Comparison");
    group.sample_size(20);

    for size in [100, 500, 1_000, 5_000, 10_000] {
        let state = generate_test_data(size);

        // Convert to vec format for ordered_trie_root_with_encoder
        let entries: Vec<(B256, U256)> = state.into_iter().collect();

        // Benchmark alloy_trie::ordered_trie_root_with_encoder
        group.bench_function(BenchmarkId::new("alloy_trie::ordered_trie_root", size), |b| {
            b.iter(|| {
                ordered_trie_root_with_encoder(&entries, |(k, v), buf| {
                    k.encode(buf);
                    v.encode(buf);
                })
            })
        });

        // Benchmark sparse trie (serial)
        let provider = DefaultTrieNodeProvider;
        let state_for_sparse = B256Map::from_iter(entries.clone());
        group.bench_function(BenchmarkId::new("sparse_trie_serial", size), |b| {
            b.iter_with_setup(
                || SparseTrie::<SerialSparseTrie>::revealed_empty(),
                |mut sparse| {
                    for (key, value) in &state_for_sparse {
                        sparse
                            .update_leaf(
                                Nibbles::unpack(key),
                                alloy_rlp::encode_fixed_size(value).to_vec(),
                                &provider,
                            )
                            .unwrap();
                    }
                    sparse.root().unwrap()
                },
            )
        });

        // For parallel trie, we need a more complex setup with state
        if size <= 5_000 {
            // Parallel setup is more complex, limit size
            let state_for_parallel = B256Map::from_iter(entries.clone());
            let hashed_state = HashedPostState::default()
                .with_storages([(B256::ZERO, HashedStorage::from_iter(false, state_for_parallel))]);

            // Setup provider with initial empty state
            let provider_factory = create_test_provider_factory();
            let view = ConsistentDbView::new(provider_factory.clone(), None);

            group.bench_function(BenchmarkId::new("parallel_sparse_trie", size), |b| {
                b.iter_with_setup(
                    || {
                        ParallelStateRoot::new(
                            view.clone(),
                            TrieInput::from_state(hashed_state.clone()),
                        )
                    },
                    |calculator| calculator.incremental_root(),
                )
            });
        }
    }
}

/// Benchmark receipts root calculation specifically
pub fn receipts_root_comparison(c: &mut Criterion) {
    use alloy_consensus::ReceiptWithBloom;
    use alloy_eips::eip2718::Encodable2718;
    use proptest_arbitrary_interop::arb;
    use reth_ethereum_primitives::Receipt;
    use reth_trie::triehash::KeccakHasher;

    let mut group = c.benchmark_group("Receipts Root Comparison");

    for size in [10, 100, 1_000] {
        // Generate test receipts
        let receipts: Vec<ReceiptWithBloom<Receipt>> =
            prop::collection::vec(arb::<ReceiptWithBloom<Receipt>>(), size)
                .new_tree(&mut TestRunner::deterministic())
                .unwrap()
                .current();

        // Benchmark using triehash
        group.bench_function(BenchmarkId::new("triehash::ordered_trie_root", size), |b| {
            b.iter(|| {
                triehash::ordered_trie_root::<KeccakHasher, _>(
                    receipts.iter().map(|r| r.encoded_2718()),
                )
            })
        });

        // Benchmark using alloy_trie::ordered_trie_root_with_encoder
        group.bench_function(BenchmarkId::new("alloy_trie::ordered_trie_root", size), |b| {
            b.iter(|| ordered_trie_root_with_encoder(&receipts, |r, buf| r.encode_2718(buf)))
        });

        // Benchmark using sparse trie
        let provider = DefaultTrieNodeProvider;
        group.bench_function(BenchmarkId::new("sparse_trie_receipts", size), |b| {
            b.iter_with_setup(
                || SparseTrie::<SerialSparseTrie>::revealed_empty(),
                |mut sparse| {
                    for (i, receipt) in receipts.iter().enumerate() {
                        let index = alloy_trie::root::adjust_index_for_rlp(i, receipts.len());
                        let mut index_buffer = Vec::new();
                        index.encode(&mut index_buffer);

                        sparse
                            .update_leaf(
                                Nibbles::unpack(&index_buffer),
                                receipt.encoded_2718().to_vec(),
                                &provider,
                            )
                            .unwrap();
                    }
                    sparse.root().unwrap()
                },
            )
        });
    }
}

fn generate_test_data(size: usize) -> B256Map<U256> {
    let mut runner = TestRunner::deterministic();
    proptest::collection::hash_map(any::<B256>(), any::<U256>(), size)
        .new_tree(&mut runner)
        .unwrap()
        .current()
        .into_iter()
        .collect()
}

criterion_group!(benches, compare_trie_implementations, receipts_root_comparison);
criterion_main!(benches);
