use criterion::{black_box, criterion_group, criterion_main, Criterion};
use proptest::{
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_primitives::{proofs::KeccakHasher, ReceiptWithBloom, H256};

/// Benchmarks different implementations of the root calculation.
pub fn trie_root_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Receipts root calculation");

    for size in [10, 100, 1_000] {
        let group_name =
            |description: &str| format!("receipts root | size: {size} | {description}");

        let (test_data, expected) = generate_test_data(size);
        use implementations::*;

        group.bench_function(group_name("triehash::ordered_trie_root"), |b| {
            b.iter(|| {
                let receipts = test_data.clone();
                let result = black_box(trie_hash_ordered_trie_root(receipts.into_iter()));
                assert_eq!(result, expected);
            });
        });

        group.bench_function(group_name("HashBuilder"), |b| {
            b.iter(|| {
                let receipts = test_data.clone();
                let result = black_box(hash_builder_root(receipts));
                assert_eq!(result, expected);
            });
        });
    }
}

fn generate_test_data(size: usize) -> (Vec<ReceiptWithBloom>, H256) {
    let receipts = prop::collection::vec(any::<ReceiptWithBloom>(), size)
        .new_tree(&mut TestRunner::new(ProptestConfig::default()))
        .unwrap()
        .current();
    let root = implementations::hash_builder_root(receipts.clone());
    (receipts, root)
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = trie_root_benchmark
}
criterion_main!(benches);

mod implementations {
    use super::*;
    use bytes::BytesMut;
    use reth_primitives::{
        proofs::adjust_index_for_rlp,
        trie::{HashBuilder, Nibbles},
    };
    use reth_rlp::Encodable;
    use std::vec::IntoIter;

    pub fn trie_hash_ordered_trie_root(receipts: IntoIter<ReceiptWithBloom>) -> H256 {
        triehash::ordered_trie_root::<KeccakHasher, _>(receipts.map(|receipt| {
            let mut receipt_rlp = Vec::new();
            receipt.encode_inner(&mut receipt_rlp, false);
            receipt_rlp
        }))
    }

    pub fn hash_builder_root(receipts: Vec<ReceiptWithBloom>) -> H256 {
        let mut index_buffer = BytesMut::new();
        let mut value_buffer = BytesMut::new();

        let mut hb = HashBuilder::default();
        let receipts_len = receipts.len();
        for i in 0..receipts_len {
            let index = adjust_index_for_rlp(i, receipts_len);

            index_buffer.clear();
            index.encode(&mut index_buffer);

            value_buffer.clear();
            receipts[index].encode_inner(&mut value_buffer, false);

            hb.add_leaf(Nibbles::unpack(&index_buffer), &value_buffer);
        }

        hb.root()
    }
}
