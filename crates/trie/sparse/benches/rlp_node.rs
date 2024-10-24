#![allow(missing_docs, unreachable_pub)]

use alloy_primitives::{B256, U256};
use criterion::{criterion_group, criterion_main, Criterion};
use prop::strategy::ValueTree;
use proptest::{prelude::*, test_runner::TestRunner};
use reth_trie::Nibbles;
use reth_trie_sparse::RevealedSparseTrie;

pub fn update_rlp_node_level(c: &mut Criterion) {
    let mut group = c.benchmark_group("update rlp node level");
    group.sample_size(20);

    for size in [1_000, 5_000, 10_000, 100_000] {
        let mut runner = TestRunner::new(ProptestConfig::default());
        let state = proptest::collection::hash_map(any::<B256>(), any::<U256>(), size)
            .new_tree(&mut runner)
            .unwrap()
            .current()
            .into_iter();

        let mut sparse = RevealedSparseTrie::default();
        for (key, value) in state {
            sparse
                .update_leaf(Nibbles::unpack(key), alloy_rlp::encode_fixed_size(&value).to_vec())
                .unwrap();
        }

        let max_depth = (size as f64).log(16.0).ceil() as usize;
        for depth in 0..=max_depth {
            group.bench_function(format!("size {size} | depth {depth}"), |b| {
                b.iter_with_setup(
                    || sparse.clone(),
                    |mut sparse| sparse.update_rlp_node_level(depth),
                )
            });
        }
    }
}

criterion_group!(rlp_node, update_rlp_node_level);
criterion_main!(rlp_node);
