#![allow(missing_docs, unreachable_pub)]

use std::time::{Duration, Instant};

use alloy_primitives::{B256, U256};
use criterion::{criterion_group, criterion_main, Criterion};
use prop::strategy::ValueTree;
use proptest::{prelude::*, test_runner::TestRunner};
use rand::seq::IteratorRandom;
use reth_testing_utils::generators;
use reth_trie::Nibbles;
use reth_trie_sparse::RevealedSparseTrie;

pub fn update_rlp_node_level(c: &mut Criterion) {
    let mut rng = generators::rng();

    let mut group = c.benchmark_group("update rlp node level");
    group.sample_size(20);

    for size in [100_000] {
        let mut runner = TestRunner::new(ProptestConfig::default());
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
                        // Use `iter_custom` to avoid measuring clones and drops
                        b.iter_custom(|iters| {
                            let mut elapsed = Duration::ZERO;

                            let mut cloned = sparse.clone();
                            for _ in 0..iters {
                                let start = Instant::now();
                                cloned.update_rlp_node_level(depth);
                                elapsed += start.elapsed();
                                cloned = sparse.clone();
                            }

                            elapsed
                        })
                    },
                );
            }
        }
    }
}

criterion_group!(rlp_node, update_rlp_node_level);
criterion_main!(rlp_node);
