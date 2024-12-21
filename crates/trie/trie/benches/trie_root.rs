#![allow(missing_docs, unreachable_pub)]
use alloy_primitives::B256;
use criterion::{criterion_group, criterion_main, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use proptest_arbitrary_interop::arb;
use reth_primitives::{Receipt, ReceiptWithBloom};
use reth_trie::triehash::KeccakHasher;
use std::hint::black_box;

/// Benchmarks different implementations of the root calculation.
pub fn trie_root_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Receipts root calculation");

    for size in [10, 100, 1_000] {
        let group_name =
            |description: &str| format!("receipts root | size: {size} | {description}");

        let receipts = &generate_test_data(size)[..];
        assert_eq!(trie_hash_ordered_trie_root(receipts), hash_builder_root(receipts));

        group.bench_function(group_name("triehash::ordered_trie_root"), |b| {
            b.iter(|| trie_hash_ordered_trie_root(black_box(receipts)));
        });

        group.bench_function(group_name("HashBuilder"), |b| {
            b.iter(|| hash_builder_root(black_box(receipts)));
        });
    }
}

fn generate_test_data(size: usize) -> Vec<ReceiptWithBloom<Receipt>> {
    prop::collection::vec(arb::<ReceiptWithBloom<Receipt>>(), size)
        .new_tree(&mut TestRunner::new(ProptestConfig::default()))
        .unwrap()
        .current()
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = trie_root_benchmark
}
criterion_main!(benches);

mod implementations {
    use super::*;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_rlp::Encodable;
    use alloy_trie::root::adjust_index_for_rlp;
    use reth_primitives::Receipt;
    use reth_trie_common::{HashBuilder, Nibbles};

    pub fn trie_hash_ordered_trie_root(receipts: &[ReceiptWithBloom<Receipt>]) -> B256 {
        triehash::ordered_trie_root::<KeccakHasher, _>(
            receipts.iter().map(|receipt_with_bloom| receipt_with_bloom.encoded_2718()),
        )
    }

    pub fn hash_builder_root(receipts: &[ReceiptWithBloom<Receipt>]) -> B256 {
        let mut index_buffer = Vec::new();
        let mut value_buffer = Vec::new();

        let mut hb = HashBuilder::default();
        let receipts_len = receipts.len();
        for i in 0..receipts_len {
            let index = adjust_index_for_rlp(i, receipts_len);

            index_buffer.clear();
            index.encode(&mut index_buffer);

            value_buffer.clear();
            receipts[index].encode_2718(&mut value_buffer);

            hb.add_leaf(Nibbles::unpack(&index_buffer), &value_buffer);
        }

        hb.root()
    }
}
use implementations::*;
