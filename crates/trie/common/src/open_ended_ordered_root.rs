//! Open-ended incremental ordered trie root computation.
//!
//! Ethereum block transaction and receipt roots are ordered trie roots keyed by the RLP encoding of
//! each item index.
//! [`OrderedTrieRootEncodedBuilder`](crate::ordered_root::OrderedTrieRootEncodedBuilder)
//! requires the total number of items up front because it accepts out-of-order items and must know
//! the final RLP key order before it can flush buffered leaves.
//!
//! [`OpenEndedOrderedTrieRootEncodedBuilder`](crate::open_ended_ordered_root::OpenEndedOrderedTrieRootEncodedBuilder)
//! is for the append-only case where items are received in their final contiguous order, but the
//! final item count is not known until the stream ends. This is useful while building a block: once
//! a transaction is committed at the next block index, its encoded transaction and receipt can be
//! consumed immediately without waiting to know how many transactions will eventually land in the
//! block.
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
//! use reth_trie_common::open_ended_ordered_root::OpenEndedOrderedTrieRootEncodedBuilder;
//!
//! let mut builder = OpenEndedOrderedTrieRootEncodedBuilder::new();
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
/// Unlike [`OrderedTrieRootEncodedBuilder`](crate::ordered_root::OrderedTrieRootEncodedBuilder),
/// this type does not require the total item count up front. It relies on a stricter input
/// contract instead: every pushed item must be the next contiguous item in the final list.
///
/// This builder is intended for transaction and receipt root computation while a block is still
/// being built. It buffers only index `0`, then streams all other leaves directly into the
/// [`HashBuilder`] in the same order as `alloy_trie::root::ordered_trie_root_encoded`.
#[derive(Debug, Default)]
pub struct OpenEndedOrderedTrieRootEncodedBuilder {
    /// Number of items pushed so far. This is also the next append-only index.
    len: usize,
    /// Index 0 is the only item whose final insertion position depends on whether more items
    /// arrive.
    zero: Option<Vec<u8>>,
    /// The underlying hash builder.
    hb: HashBuilder,
}

impl OpenEndedOrderedTrieRootEncodedBuilder {
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
        // SAFETY: RLP-encoded usize indices are at most 9 bytes on 64-bit targets, well below the
        // 32-byte limit enforced by `Nibbles::unpack`.
        self.hb.add_leaf(unsafe { Nibbles::unpack_unchecked(&index_buffer) }, bytes);
    }
}
