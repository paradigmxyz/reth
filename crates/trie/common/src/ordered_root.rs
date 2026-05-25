//! Incremental ordered trie root computation for append-only streams.
//!
//! Ethereum block transaction and receipt roots are ordered trie roots keyed by the RLP encoding of
//! each item index. [`OrderedTrieRootEncodedBuilder`] accepts items in their final contiguous
//! order, without requiring the final item count until the stream ends.
//!
//! # Why this works
//!
//! RLP-encoded integer keys sort differently around small indices:
//!
//! - indices `1..=127` encode as single bytes `0x01..=0x7f`;
//! - index `0` encodes as `0x80`, so it sorts after `1..=127`;
//! - indices `>= 128` encode as longer byte strings and sort after `0`.
//!
//! This means the final insertion order for a dense list is:
//!
//! ```text
//! len = 0:      []
//! len = 1:      [0]
//! 2..=128:     [1, 2, ..., len - 1, 0]
//! >=129:       [1, 2, ..., 127, 0, 128, 129, ...]
//! ```
//!
//! While the stream is open, the only ambiguous leaf is index `0`: for short lists it is inserted
//! when the stream ends, and for lists with at least 129 items it can be inserted as soon as index
//! `128` arrives. All other leaves can be added as they arrive in append-only order.
//!
//! # Example
//!
//! ```
//! use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;
//!
//! let mut builder = OrderedTrieRootEncodedBuilder::new();
//! builder.push_next(b"encoded_item_0");
//! builder.push_next(b"encoded_item_1");
//!
//! let root = builder.finalize();
//! ```

use crate::{HashBuilder, Nibbles, EMPTY_ROOT_HASH};
use alloc::vec::Vec;
use alloy_primitives::B256;

/// First index whose RLP-encoded key sorts after index 0.
///
/// Indices `1..=0x7f` encode as their own single byte, so they sort before the RLP encoding of
/// index 0 (`0x80`). Index `0x80` and larger use long-form RLP integer encoding and sort after
/// index 0, so index 0 must be flushed before inserting this index.
const ZERO_KEY_FLUSH_INDEX: usize = 0x80;

/// A builder for computing ordered trie roots from an append-only stream of pre-encoded items.
///
/// This type does not require the total item count up front. It relies on a strict input contract
/// instead: every pushed item must be the next contiguous item in the final list.
///
/// This builder is intended for transaction and receipt root computation while a block is still
/// being built. It buffers only index `0`, then streams all other leaves directly into the
/// [`HashBuilder`] in the same order as `alloy_trie::root::ordered_trie_root_encoded`.
#[derive(Debug, Default)]
pub struct OrderedTrieRootEncodedBuilder {
    /// Number of items pushed so far. This is also the next append-only index.
    len: usize,
    /// Index 0 is the only item whose final insertion position depends on whether more items
    /// arrive.
    zero: Option<Vec<u8>>,
    /// The underlying hash builder.
    hb: HashBuilder,
}

impl OrderedTrieRootEncodedBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes the next pre-encoded item.
    ///
    /// Items must be pushed in their final contiguous order.
    #[inline]
    pub fn push_next(&mut self, bytes: &[u8]) {
        let index = self.len;
        self.len += 1;

        match index {
            0 => {
                self.zero = Some(bytes.to_vec());
            }
            1..=0x7f => {
                self.add_leaf(index, bytes);
            }
            ZERO_KEY_FLUSH_INDEX => {
                self.flush_zero();
                self.add_leaf(index, bytes);
            }
            _ => {
                debug_assert!(self.zero.is_none(), "index 0 must be flushed before indices > 128");
                self.add_leaf(index, bytes);
            }
        }
    }

    /// Returns the number of items pushed so far.
    #[inline]
    pub const fn pushed_count(&self) -> usize {
        self.len
    }

    /// Returns `true` if no items have been pushed.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Finalizes the builder and returns the trie root.
    ///
    /// This method is the end-of-stream signal. If fewer than 129 items were pushed, index `0` is
    /// inserted here because it is the final leaf in RLP key order for that short list.
    pub fn finalize(mut self) -> B256 {
        if self.len == 0 {
            return EMPTY_ROOT_HASH;
        }

        self.flush_zero();
        self.hb.root()
    }

    fn flush_zero(&mut self) {
        if self.zero.is_none() {
            return;
        }

        let zero = self.zero.take().expect("index 0 must be buffered before it is flushed");
        self.add_leaf(0, &zero);
    }

    fn add_leaf(&mut self, index: usize, bytes: &[u8]) {
        let index_buffer = alloy_rlp::encode_fixed_size(&index);
        self.hb.add_leaf(Nibbles::unpack(&index_buffer), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_trie::root::ordered_trie_root_encoded;
    use proptest::prelude::*;

    fn item(index: usize) -> Vec<u8> {
        format!("encoded_item_{index}").into_bytes()
    }

    fn items(len: usize) -> Vec<Vec<u8>> {
        (0..len).map(item).collect()
    }

    fn root_with_push_next(items: &[Vec<u8>]) -> B256 {
        let mut builder = OrderedTrieRootEncodedBuilder::new();
        for item in items {
            builder.push_next(item);
        }
        builder.finalize()
    }

    #[test]
    fn test_ordered_encoded_builder_equivalence() {
        for len in [0, 1, 2, 3, 10, 127, 128, 129, 130, 200] {
            let items: Vec<Vec<u8>> =
                (0..len).map(|i| format!("item_{i}_data").into_bytes()).collect();

            let expected = ordered_trie_root_encoded(&items);

            let mut builder = OrderedTrieRootEncodedBuilder::new();

            for item in &items {
                builder.push_next(item);
            }

            let actual = builder.finalize();
            assert_eq!(
                expected, actual,
                "mismatch for len={len}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn empty_builder_returns_empty_root() {
        let builder = OrderedTrieRootEncodedBuilder::new();
        assert!(builder.is_empty());
        assert_eq!(builder.pushed_count(), 0);
        assert_eq!(builder.finalize(), EMPTY_ROOT_HASH);
    }

    #[test]
    fn test_ordered_builder_count_tracks_pushed_items() {
        let mut builder = OrderedTrieRootEncodedBuilder::new();
        assert!(builder.is_empty());

        builder.push_next(b"item_0");
        assert!(!builder.is_empty());
        assert_eq!(builder.pushed_count(), 1);

        builder.push_next(b"item_1");
        assert_eq!(builder.pushed_count(), 2);
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

            let mut builder = OrderedTrieRootEncodedBuilder::new();
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

        let mut builder = OrderedTrieRootEncodedBuilder::new();
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
            let mut builder = OrderedTrieRootEncodedBuilder::new();
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
}
