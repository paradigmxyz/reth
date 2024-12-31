#![allow(missing_docs)]

use alloy_primitives::{B256, U256};
use criterion::{criterion_group, criterion_main, Criterion};
use prop::strategy::ValueTree;
use proptest::{prelude::*, test_runner::TestRunner};
use rand::seq::IteratorRandom;
use reth_testing_utils::generators;
use reth_trie::Nibbles;
use reth_trie_sparse::RevealedSparseTrie;

fn update_rlp_node_level(c: &mut Criterion) {
    let mut rng = generators::rng_with_seed(&12345_u16.to_be_bytes());
    let mut group = c.benchmark_group("update rlp node level");
    group.sample_size(20);

    for size in [100_000] {
        let mut runner = TestRunner::deterministic();
        let state = proptest::collection::hash_map(any::<B256>(), any::<U256>(), size)
            .new_tree(&mut runner)
            .unwrap()
            .current();

        // Create a sparse trie with `size` leaves
        let mut sparse = RevealedSparseTrie::default();
        for (key, value) in &state {
            sparse
                .update_leaf(Nibbles::unpack(key), alloy_rlp::encode_fixed_size(value).to_vec())
                .unwrap();
        }
        sparse.root();

        for updated_leaves in [0.1, 1.0] {
            for key in state
                .keys()
                .choose_multiple(&mut rng, (size as f64 * (updated_leaves / 100.0)) as usize)
            {
                sparse
                    .update_leaf(
                        Nibbles::unpack(key),
                        alloy_rlp::encode_fixed_size(&rng.gen::<U256>()).to_vec(),
                    )
                    .unwrap();
            }

            // Calculate the maximum depth of the trie for the given number of leaves
            let max_depth = (size as f64).log(16.0).ceil() as usize;

            for depth in 0..=max_depth {
                group.bench_function(
                    format!("size {size} | updated {updated_leaves}% | depth {depth}"),
                    |b| {
                        b.iter_batched_ref(
                            || sparse.clone(),
                            |cloned| cloned.update_rlp_node_level(depth),
                            criterion::BatchSize::PerIteration,
                        )
                    },
                );
            }
        }
    }
}

criterion_group!(rlp_node, update_rlp_node_level);
criterion_main!(rlp_node);
