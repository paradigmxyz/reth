//! Tests for open-ended ordered trie root computation.

use alloy_primitives::B256;
use alloy_trie::root::ordered_trie_root_encoded;
use proptest::prelude::*;
use reth_trie_common::{
    open_ended_ordered_root::OpenEndedOrderedTrieRootEncodedBuilder, EMPTY_ROOT_HASH,
};

fn item(index: usize) -> Vec<u8> {
    format!("encoded_item_{index}").into_bytes()
}

fn items(len: usize) -> Vec<Vec<u8>> {
    (0..len).map(item).collect()
}

fn root_with_push_next(items: &[Vec<u8>]) -> B256 {
    let mut builder = OpenEndedOrderedTrieRootEncodedBuilder::new();
    for item in items {
        builder.push_next(item);
    }
    builder.finalize()
}

#[test]
fn empty_builder_returns_empty_root() {
    let builder = OpenEndedOrderedTrieRootEncodedBuilder::new();

    assert!(builder.is_empty());
    assert_eq!(builder.pushed_count(), 0);
    assert_eq!(builder.finalize(), EMPTY_ROOT_HASH);
}

#[test]
fn matches_ordered_root_for_boundary_lengths() {
    for len in [0, 1, 2, 3, 4, 126, 127, 128, 129, 130, 200, 255, 256, 512] {
        let items = items(len);
        let expected = ordered_trie_root_encoded(&items);

        assert_eq!(root_with_push_next(&items), expected, "push_next mismatch for len={len}");
    }
}

#[test]
fn short_lists_flush_zero_on_finalize() {
    for len in 1..=128 {
        let items = items(len);
        let expected = ordered_trie_root_encoded(&items);

        let mut builder = OpenEndedOrderedTrieRootEncodedBuilder::new();
        for item in &items {
            builder.push_next(item);
        }

        assert_eq!(builder.pushed_count(), len);
        assert_eq!(builder.finalize(), expected, "mismatch for short len={len}");
    }
}

#[test]
fn long_lists_flush_zero_when_index_128_arrives() {
    let items = items(129);
    let expected = ordered_trie_root_encoded(&items);

    let mut builder = OpenEndedOrderedTrieRootEncodedBuilder::new();
    for item in &items {
        builder.push_next(item);
    }

    assert_eq!(builder.pushed_count(), 129);
    assert_eq!(builder.finalize(), expected);
}

proptest! {
    #[test]
    fn arbitrary_encoded_items_match_ordered_root(items in proptest::collection::vec(
        proptest::collection::vec(any::<u8>(), 0..128),
        0..1024,
    )) {
        let expected = ordered_trie_root_encoded(&items);

        prop_assert_eq!(root_with_push_next(&items), expected);
    }
}

#[cfg(feature = "arbitrary")]
mod arbitrary_consensus_roots {
    use super::*;
    use alloy_consensus::{
        proofs::{calculate_receipt_root, calculate_transaction_root},
        EthereumReceipt, ReceiptWithBloom, Signed, TxLegacy,
    };
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::Signature;
    use proptest::test_runner::Config;
    use proptest_arbitrary_interop::arb;

    fn open_ended_2718_root<T: Encodable2718>(items: &[T]) -> B256 {
        let mut builder = OpenEndedOrderedTrieRootEncodedBuilder::new();
        let mut buf = Vec::new();

        for item in items {
            buf.clear();
            item.encode_2718(&mut buf);
            builder.push_next(&buf);
        }

        builder.finalize()
    }

    proptest! {
        #![proptest_config(Config::with_cases(32))]

        #[test]
        fn arbitrary_transactions_match_alloy_consensus_root(
            transactions in proptest::collection::vec(
                (arb::<TxLegacy>(), arb::<Signature>())
                    .prop_map(|(tx, signature)| Signed::new_unhashed(tx, signature)),
                0..1024,
            ),
        ) {
            let expected = calculate_transaction_root(&transactions);
            let actual = open_ended_2718_root(&transactions);

            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn arbitrary_receipts_match_alloy_consensus_root(
            receipts in proptest::collection::vec(
                arb::<ReceiptWithBloom<EthereumReceipt>>(),
                0..1024,
            ),
        ) {
            let expected = calculate_receipt_root(&receipts);
            let actual = open_ended_2718_root(&receipts);

            prop_assert_eq!(actual, expected);
        }
    }
}
